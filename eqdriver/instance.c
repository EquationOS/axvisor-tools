#include <linux/fs.h>
#include <linux/atomic.h>
#include <linux/eventfd.h>
#include <linux/interrupt.h>
#include <linux/io.h>
#include <linux/irq.h>
#include <linux/irqbypass.h>
#include <linux/list.h>
#include <linux/miscdevice.h>
#include <linux/mm.h>
#include <linux/slab.h>
#include <linux/mutex.h>
#include <linux/moduleparam.h>
#include <linux/pgtable.h>
#include <linux/uaccess.h>
#include <linux/pid.h>	 // for pid_nr()
#include <linux/sched.h> // for current
#include <linux/sched/mm.h>
#include <linux/uaccess.h>
#include <linux/hashtable.h>

#ifdef CONFIG_X86
#include <asm/irq_remapping.h>
#endif

#include "includes/eqmanager.h"
#include "includes/hvc.h"
#include "includes/instance.h"
#include "includes/utils.h"

typedef struct eq_irq_route
{
	struct list_head list;
	struct irq_bypass_consumer consumer;
	struct eventfd_ctx *eventfd;
	uint64_t instance_id;
	uint32_t msix_index;
	uint32_t target_vcpu;
	uint32_t guest_vector;
	uint32_t posted_vector;
	uint64_t pi_desc_hpa;
	int host_irq;
	bool posted_active;
	bool posted_shared_pid;
	bool posted_vector_hardware;
	bool logged_not_ready;
	struct list_head posted_owner_list;
	bool posted_owner_linked;
} eq_irq_route_t;

typedef struct eq_vfio_posted_owner
{
	struct hlist_node node;
	uint32_t target_vcpu;
	uint64_t owner_instance_id;
	struct list_head active_routes;
} eq_vfio_posted_owner_t;

#define EQ_VFIO_POSTED_OWNER_BITS 4
#define EQ_IRQ_ROUTE_FLAG_COMMIT_OWNER (1U << 0)
#define EQ_IRQ_ROUTE_FLAG_SHARED_VMCS_PID (1U << 1)
#define EQ_IRQ_ROUTE_FLAG_HAS_POSTED_VECTOR (1U << 2)
#define EQ_IRQ_ROUTE_FLAG_REQUIRES_POSTED_VECTOR (1U << 3)
#define EQ_MICROVM_BLOCK_FLAG_ENABLED (1ULL << 0)
#define MICROVM_BLOCK_NOTIFY_VERSION (1U)
#define EQ_HYPERALLOC_VFIO_DMA_STATUS_NONE (0U)
#define EQ_HYPERALLOC_VFIO_DMA_STATUS_PENDING (1U)
#define EQ_HYPERALLOC_VFIO_DMA_STATUS_UNSUPPORTED (4U)
#define MICROVM_LOW_RAM_LIMIT (1ULL << 30)
#define MICROVM_HIGH_RAM_START (6ULL << 30)

static bool eq_guest_mem_copy_direction_valid(uint32_t flags)
{
	uint32_t direction =
		flags & (EQ_MICROVM_GUEST_MEM_COPY_READ_FROM_GUEST |
			 EQ_MICROVM_GUEST_MEM_COPY_WRITE_TO_GUEST);

	return direction == EQ_MICROVM_GUEST_MEM_COPY_READ_FROM_GUEST ||
	       direction == EQ_MICROVM_GUEST_MEM_COPY_WRITE_TO_GUEST;
}

static DEFINE_HASHTABLE(eq_vfio_posted_owners, EQ_VFIO_POSTED_OWNER_BITS);
static DEFINE_MUTEX(eq_vfio_posted_owners_lock);
static bool eq_vfio_use_eqgate_posted_vector;
module_param(eq_vfio_use_eqgate_posted_vector, bool, 0644);
MODULE_PARM_DESC(
	eq_vfio_use_eqgate_posted_vector,
	"Use hypervisor-provided owner-coded posted vector for VFIO IRQ posting");

typedef struct microvm_block_notify_page_header
{
	uint32_t magic;
	uint32_t version;
	uint32_t size;
	uint32_t flags;
	uint32_t head;
	uint32_t tail;
	uint32_t dropped;
} microvm_block_notify_page_header_t;

static void microvm_block_notify_ring_init(void *page)
{
	microvm_block_notify_page_header_t *ring =
		(microvm_block_notify_page_header_t *)page;

	memset(page, 0, PAGE_SIZE);
	ring->magic = MMAP_MICROVM_BLOCK_NOTIFY_MAGIC_NUMBER;
	ring->version = MICROVM_BLOCK_NOTIFY_VERSION;
	ring->size = PAGE_SIZE;
}

static void eq_irq_route_deactivate_owned_locked(eq_irq_route_t *route, int producer_irq, const char *reason);

typedef struct eq_instance_vdev
{
	struct miscdevice misc;
	char name[64];
	/// @brief Unique identifier for the instance.
	/// This ID is assigned by the hypervisor and is used to identify the
	/// instance.
	int id;
	/// @brief Active status of the instance.
	/// If true, this `eq_instance_vdev_t` is active and can be used.
	/// If false, this `eq_instance_vdev_t` is not active and cannot be used.
	bool active;
	/// @brief Running status of the instance.
	/// - 0: Just created, not running yet.
	/// - 1: Setting up, not running yet.
	/// - 2: Running, the instance is running.
	int status;

	/// @brief Type of the instance.
	/// - 0: staticlly linked LibOS instance.
	/// - 1: dynamically linked LibOS instance.
	/// - 2: microVM instance.
	int instance_type;

	/// @brief Backing page for the microVM PV console ring.
	void *microvm_console_ring_virt;
	/// @brief Backing page for split virtio-blk queue notifications.
	void *microvm_block_notify_ring_virt;
	struct list_head irq_routes;
	struct mutex irq_routes_lock;
	struct mutex microvm_guest_ram_mmap_lock;
	struct list_head microvm_guest_ram_vma_list;
	uint64_t microvm_guest_ram_mmap_generation;
	atomic_t microvm_guest_ram_mmap_count;
	atomic_t microvm_guest_ram_current_mmap_count;
	atomic_t microvm_guest_ram_stale_mmap_count;
	atomic64_t microvm_guest_ram_mmap_update_seq;

	eq_instance_metadata_t metadata;
} eq_instance_vdev_t;

static eq_instance_vdev_t instances_array[MAX_EQ_INSTANCES_NUM];
static bool instances_exiting;

int unregister_instance_dev(eq_instance_vdev_t *vdev);

typedef struct eq_instance_file_context
{
	eq_instance_vdev_t *instance_vdev;
	uint64_t generation;
} eq_instance_file_context_t;

typedef struct microvm_guest_ram_vma_context
{
	eq_instance_vdev_t *instance_vdev;
	struct mm_struct *mm;
	struct list_head list;
	uint64_t generation;
	uint64_t gpa_start;
	uint64_t size;
	unsigned long vm_start;
	unsigned long vm_end;
	bool listed;
	atomic_t refs;
} microvm_guest_ram_vma_context_t;

typedef struct microvm_guest_ram_generation_zap_target
{
	microvm_guest_ram_vma_context_t *ctx;
	unsigned long va_start;
	unsigned long size;
} microvm_guest_ram_generation_zap_target_t;

static bool microvm_guest_ram_file_offset_to_gpa(
	eq_instance_vdev_t *instance_vdev, uint64_t file_offset, uint64_t len,
	uint64_t *gpa)
{
	uint64_t mem_size;
	uint64_t end_offset;

	if (!instance_vdev || len == 0 || !gpa)
		return false;
	mem_size = instance_vdev->metadata.init_memory_region_size_mib * 1024ULL * 1024ULL;
	if (__builtin_add_overflow(file_offset, len - 1, &end_offset))
		return false;
	if (end_offset >= mem_size)
		return false;
	if (file_offset < MICROVM_LOW_RAM_LIMIT)
	{
		if (end_offset >= MICROVM_LOW_RAM_LIMIT && mem_size > MICROVM_LOW_RAM_LIMIT)
			return false;
		*gpa = file_offset;
		return true;
	}
	*gpa = MICROVM_HIGH_RAM_START + (file_offset - MICROVM_LOW_RAM_LIMIT);
	return true;
}

static bool microvm_guest_ram_gpa_range_valid(
	eq_instance_vdev_t *instance_vdev, uint64_t gpa, uint64_t len)
{
	uint64_t mem_size;
	uint64_t end_gpa;
	uint64_t low_size;
	uint64_t high_size;

	if (!instance_vdev || len == 0)
		return false;
	mem_size = instance_vdev->metadata.init_memory_region_size_mib * 1024ULL * 1024ULL;
	if (__builtin_add_overflow(gpa, len - 1, &end_gpa))
		return false;
	low_size = min(mem_size, MICROVM_LOW_RAM_LIMIT);
	if (gpa < MICROVM_LOW_RAM_LIMIT)
		return gpa < low_size && end_gpa < low_size;
	if (mem_size <= MICROVM_LOW_RAM_LIMIT || gpa < MICROVM_HIGH_RAM_START)
		return false;
	high_size = mem_size - MICROVM_LOW_RAM_LIMIT;
	if (gpa - MICROVM_HIGH_RAM_START >= high_size)
		return false;
	return end_gpa >= MICROVM_HIGH_RAM_START &&
	       end_gpa - MICROVM_HIGH_RAM_START < high_size;
}

static void microvm_guest_ram_vma_context_get(
	microvm_guest_ram_vma_context_t *ctx)
{
	if (ctx)
		atomic_inc(&ctx->refs);
}

static void microvm_guest_ram_vma_context_put(
	microvm_guest_ram_vma_context_t *ctx)
{
	int refs;

	if (!ctx)
		return;
	refs = atomic_dec_return(&ctx->refs);
	if (refs < 0)
	{
		WARNING(
			"MicroVM guest RAM VMA context ref underflow generation=%llu\n",
			(unsigned long long)ctx->generation);
		atomic_set(&ctx->refs, 0);
		refs = 0;
	}
	if (refs == 0)
	{
		if (ctx->mm)
			mmdrop(ctx->mm);
		kfree(ctx);
	}
}

static uint64_t instance_generation_from_file(struct file *file)
{
	eq_instance_file_context_t *ctx = file->private_data;

	return ctx ? ctx->generation : 0;
}

static eq_instance_vdev_t *active_instance_vdev_from_file(struct file *file)
{
	eq_instance_file_context_t *ctx = file->private_data;
	eq_instance_vdev_t *instance_vdev;

	if (!ctx || !ctx->instance_vdev)
		return NULL;

	instance_vdev = ctx->instance_vdev;
	if (!instance_vdev->active)
		return NULL;
	if (ctx->generation != instance_vdev->microvm_guest_ram_mmap_generation)
		return NULL;
	return instance_vdev;
}

static void microvm_guest_ram_mmap_snapshot_locked(
	eq_instance_vdev_t *instance_vdev,
	uint64_t *generation,
	int *active_mmaps,
	int *current_mmaps,
	int *stale_mmaps)
{
	*generation = instance_vdev->microvm_guest_ram_mmap_generation;
	*active_mmaps = atomic_read(&instance_vdev->microvm_guest_ram_mmap_count);
	*current_mmaps =
		atomic_read(&instance_vdev->microvm_guest_ram_current_mmap_count);
	*stale_mmaps =
		atomic_read(&instance_vdev->microvm_guest_ram_stale_mmap_count);
}

static void microvm_guest_ram_report_state(
	eq_instance_vdev_t *instance_vdev, uint32_t reason)
{
	eq_microvm_guest_ram_mmap_state_update_t *update;
	uint64_t generation;
	int active_mmaps;
	int current_mmaps;
	int stale_mmaps;
	int ret;

	if (!instance_vdev || instance_vdev->instance_type != 2)
		return;

	mutex_lock(&instance_vdev->microvm_guest_ram_mmap_lock);
	microvm_guest_ram_mmap_snapshot_locked(
		instance_vdev, &generation, &active_mmaps, &current_mmaps,
		&stale_mmaps);
	mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);

	if (instances_exiting || !instance_vdev->active)
	{
		if (reason == EQ_MICROVM_GUEST_RAM_MMAP_STATE_REASON_UNREGISTER ||
			instances_exiting)
			INFO(
				"MicroVM instance %d guest RAM mmap state HVC update skipped reason=%u active=%d exiting=%d active_mmaps=%d current_mmaps=%d stale_mmaps=%d\n",
				instance_vdev->id, reason, instance_vdev->active,
				instances_exiting, active_mmaps, current_mmaps,
				stale_mmaps);
		return;
	}

	update = kzalloc(sizeof(*update), GFP_KERNEL);
	if (!update)
	{
		WARNING(
			"MicroVM instance %d guest RAM mmap state allocation failed reason=%u active_mmaps=%d current_mmaps=%d stale_mmaps=%d\n",
			instance_vdev->id, reason, active_mmaps, current_mmaps,
			stale_mmaps);
		return;
	}

	update->version = EQ_MICROVM_GUEST_RAM_MMAP_STATE_UPDATE_VERSION;
	update->reason = reason;
	update->sequence =
		(uint64_t)atomic64_inc_return(
			&instance_vdev->microvm_guest_ram_mmap_update_seq);
	update->instance_id = (uint64_t)instance_vdev->id;
	update->generation = generation;
	update->active_mmaps = (uint64_t)active_mmaps;
	update->current_mmaps = (uint64_t)current_mmaps;
	update->stale_mmaps = (uint64_t)stale_mmaps;

	ret = hvc_microvm_guest_ram_mmap_state_update(virt_to_phys(update));
	if (ret < 0)
		WARNING(
			"MicroVM instance %d guest RAM mmap state HVC update failed reason=%u seq=%llu ret=%d active_mmaps=%d current_mmaps=%d stale_mmaps=%d\n",
			instance_vdev->id, reason,
			(unsigned long long)update->sequence, ret, active_mmaps,
			current_mmaps, stale_mmaps);
	kfree(update);
}

static void microvm_guest_ram_mmap_account(
	microvm_guest_ram_vma_context_t *ctx,
	const char *reason_name,
	uint32_t reason,
	unsigned long vm_start,
	unsigned long vm_end)
{
	eq_instance_vdev_t *instance_vdev;
	uint64_t current_generation;
	int active_mmaps;
	int current_mmaps;
	int stale_mmaps;
	bool is_current_generation;

	if (!ctx || !ctx->instance_vdev)
		return;

	instance_vdev = ctx->instance_vdev;
	mutex_lock(&instance_vdev->microvm_guest_ram_mmap_lock);
	current_generation = instance_vdev->microvm_guest_ram_mmap_generation;
	is_current_generation = ctx->generation == current_generation;
	active_mmaps =
		atomic_inc_return(&instance_vdev->microvm_guest_ram_mmap_count);
	if (is_current_generation)
		current_mmaps = atomic_inc_return(
			&instance_vdev->microvm_guest_ram_current_mmap_count);
	else
		current_mmaps =
			atomic_read(&instance_vdev->microvm_guest_ram_current_mmap_count);
	if (is_current_generation)
		stale_mmaps =
			atomic_read(&instance_vdev->microvm_guest_ram_stale_mmap_count);
	else
		stale_mmaps = atomic_inc_return(
			&instance_vdev->microvm_guest_ram_stale_mmap_count);
	mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);

	INFO(
		"MicroVM instance %d guest RAM VMA %s generation=%llu current_generation=%llu active_mmaps=%d current_mmaps=%d stale_mmaps=%d gpa[0x%llx-0x%llx] va[0x%lx-0x%lx]\n",
		instance_vdev->id, reason_name,
		(unsigned long long)ctx->generation,
		(unsigned long long)current_generation, active_mmaps, current_mmaps,
		stale_mmaps, (unsigned long long)ctx->gpa_start,
		(unsigned long long)(ctx->gpa_start + ctx->size), vm_start, vm_end);
	microvm_guest_ram_report_state(instance_vdev, reason);
}

static int microvm_guest_ram_atomic_dec_nonnegative(
	atomic_t *counter,
	const char *name,
	int instance_id)
{
	int value = atomic_dec_return(counter);

	if (value < 0)
	{
		WARNING(
			"MicroVM instance %d guest RAM mmap %s underflow value=%d\n",
			instance_id, name, value);
		atomic_set(counter, 0);
		value = 0;
	}
	return value;
}

static void microvm_guest_ram_mmap_unaccount(
	microvm_guest_ram_vma_context_t *ctx,
	const char *reason_name,
	uint32_t reason,
	unsigned long vm_start,
	unsigned long vm_end)
{
	eq_instance_vdev_t *instance_vdev;
	uint64_t current_generation;
	int active_mmaps;
	int current_mmaps;
	int stale_mmaps;
	bool is_current_generation;

	if (!ctx || !ctx->instance_vdev)
		return;

	instance_vdev = ctx->instance_vdev;
	mutex_lock(&instance_vdev->microvm_guest_ram_mmap_lock);
	current_generation = instance_vdev->microvm_guest_ram_mmap_generation;
	is_current_generation = ctx->generation == current_generation;
	active_mmaps = microvm_guest_ram_atomic_dec_nonnegative(
		&instance_vdev->microvm_guest_ram_mmap_count, "active",
		instance_vdev->id);
	if (is_current_generation)
		current_mmaps = microvm_guest_ram_atomic_dec_nonnegative(
			&instance_vdev->microvm_guest_ram_current_mmap_count, "current",
			instance_vdev->id);
	else
		current_mmaps =
			atomic_read(&instance_vdev->microvm_guest_ram_current_mmap_count);
	if (is_current_generation)
		stale_mmaps =
			atomic_read(&instance_vdev->microvm_guest_ram_stale_mmap_count);
	else
		stale_mmaps = microvm_guest_ram_atomic_dec_nonnegative(
			&instance_vdev->microvm_guest_ram_stale_mmap_count, "stale",
			instance_vdev->id);
	mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);

	INFO(
		"MicroVM instance %d guest RAM VMA %s generation=%llu current_generation=%llu active_mmaps=%d current_mmaps=%d stale_mmaps=%d gpa[0x%llx-0x%llx] va[0x%lx-0x%lx]\n",
		instance_vdev->id, reason_name,
		(unsigned long long)ctx->generation,
		(unsigned long long)current_generation, active_mmaps, current_mmaps,
		stale_mmaps, (unsigned long long)ctx->gpa_start,
		(unsigned long long)(ctx->gpa_start + ctx->size), vm_start, vm_end);
	microvm_guest_ram_report_state(instance_vdev, reason);
}

static int microvm_guest_ram_collect_generation_zap_targets(
	eq_instance_vdev_t *instance_vdev,
	uint64_t current_generation,
	microvm_guest_ram_generation_zap_target_t **targets_out,
	uint64_t *target_count_out)
{
	microvm_guest_ram_vma_context_t *ctx;
	microvm_guest_ram_generation_zap_target_t *targets;
	uint64_t count = 0;
	uint64_t idx = 0;

	*targets_out = NULL;
	*target_count_out = 0;
	mutex_lock(&instance_vdev->microvm_guest_ram_mmap_lock);
	list_for_each_entry(ctx, &instance_vdev->microvm_guest_ram_vma_list, list)
	{
		if (ctx->generation != current_generation)
			count++;
	}
	if (count == 0)
	{
		mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);
		return 0;
	}
	targets = kcalloc(count, sizeof(*targets), GFP_KERNEL);
	if (!targets)
	{
		mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);
		return -ENOMEM;
	}
	list_for_each_entry(ctx, &instance_vdev->microvm_guest_ram_vma_list, list)
	{
		if (ctx->generation == current_generation)
			continue;
		microvm_guest_ram_vma_context_get(ctx);
		targets[idx].ctx = ctx;
		targets[idx].va_start = ctx->vm_start;
		targets[idx].size = (unsigned long)ctx->size;
		idx++;
	}
	mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);

	*targets_out = targets;
	*target_count_out = idx;
	return 0;
}

static void microvm_guest_ram_put_generation_zap_targets(
	microvm_guest_ram_generation_zap_target_t *targets,
	uint64_t target_count)
{
	uint64_t i;

	if (!targets)
		return;
	for (i = 0; i < target_count; i++)
		microvm_guest_ram_vma_context_put(targets[i].ctx);
	kfree(targets);
}

static void microvm_guest_ram_zap_generation_targets(
	eq_instance_vdev_t *instance_vdev,
	uint64_t current_generation,
	microvm_guest_ram_generation_zap_target_t *targets,
	uint64_t target_count)
{
	uint64_t i;
	uint64_t zapped_vmas = 0;
	uint64_t zapped_bytes = 0;

	for (i = 0; i < target_count; i++)
	{
		microvm_guest_ram_vma_context_t *ctx = targets[i].ctx;
		struct vm_area_struct *vma;
		unsigned long va_start = targets[i].va_start;
		unsigned long va_end = va_start + targets[i].size;

		if (!ctx || !ctx->mm || targets[i].size == 0)
			continue;
		mmap_write_lock(ctx->mm);
		vma = find_vma(ctx->mm, va_start);
		if (vma && vma->vm_start <= va_start && vma->vm_end >= va_end &&
			vma->vm_private_data == ctx)
		{
			zap_vma_ptes(vma, va_start, targets[i].size);
			zapped_vmas++;
			zapped_bytes += targets[i].size;
		}
		mmap_write_unlock(ctx->mm);
	}

	if (target_count != 0)
		INFO(
			"MicroVM instance %d guest RAM generation advance stale zap current_generation=%llu targets=%llu zapped_vmas=%llu zapped_bytes=0x%llx\n",
			instance_vdev->id, (unsigned long long)current_generation,
			(unsigned long long)target_count,
			(unsigned long long)zapped_vmas,
			(unsigned long long)zapped_bytes);
}

static int microvm_guest_ram_collect_all_zap_targets(
	eq_instance_vdev_t *instance_vdev,
	microvm_guest_ram_generation_zap_target_t **targets_out,
	uint64_t *target_count_out)
{
	microvm_guest_ram_vma_context_t *ctx;
	microvm_guest_ram_generation_zap_target_t *targets;
	uint64_t count = 0;
	uint64_t idx = 0;

	*targets_out = NULL;
	*target_count_out = 0;
	mutex_lock(&instance_vdev->microvm_guest_ram_mmap_lock);
	list_for_each_entry(ctx, &instance_vdev->microvm_guest_ram_vma_list, list)
		count++;
	if (count == 0)
	{
		mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);
		return 0;
	}
	targets = kcalloc(count, sizeof(*targets), GFP_KERNEL);
	if (!targets)
	{
		mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);
		return -ENOMEM;
	}
	list_for_each_entry(ctx, &instance_vdev->microvm_guest_ram_vma_list, list)
	{
		microvm_guest_ram_vma_context_get(ctx);
		targets[idx].ctx = ctx;
		targets[idx].va_start = ctx->vm_start;
		targets[idx].size = (unsigned long)ctx->size;
		idx++;
	}
	mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);

	*targets_out = targets;
	*target_count_out = idx;
	return 0;
}

static void microvm_guest_ram_zap_unregister_vmas(
	eq_instance_vdev_t *instance_vdev)
{
	microvm_guest_ram_generation_zap_target_t *targets = NULL;
	uint64_t target_count = 0;
	uint64_t zapped_vmas = 0;
	uint64_t zapped_bytes = 0;
	uint64_t i;
	int ret;

	ret = microvm_guest_ram_collect_all_zap_targets(
		instance_vdev, &targets, &target_count);
	if (ret)
	{
		WARNING(
			"MicroVM instance %d failed to collect unregister guest RAM VMA zap targets ret=%d\n",
			instance_vdev->id, ret);
		microvm_guest_ram_report_state(
			instance_vdev,
			EQ_MICROVM_GUEST_RAM_MMAP_STATE_REASON_UNREGISTER);
		return;
	}

	for (i = 0; i < target_count; i++)
	{
		microvm_guest_ram_vma_context_t *ctx = targets[i].ctx;
		struct vm_area_struct *vma;
		unsigned long va_start = targets[i].va_start;
		unsigned long va_end = va_start + targets[i].size;

		if (!ctx || !ctx->mm || targets[i].size == 0)
			continue;
		mmap_write_lock(ctx->mm);
		vma = find_vma(ctx->mm, va_start);
		if (vma && vma->vm_start <= va_start && vma->vm_end >= va_end &&
			vma->vm_private_data == ctx)
		{
			zap_vma_ptes(vma, va_start, targets[i].size);
			zapped_vmas++;
			zapped_bytes += targets[i].size;
		}
		mmap_write_unlock(ctx->mm);
	}

	if (target_count != 0)
		INFO(
			"MicroVM instance %d guest RAM unregister stale-fence zap targets=%llu zapped_vmas=%llu zapped_bytes=0x%llx\n",
			instance_vdev->id, (unsigned long long)target_count,
			(unsigned long long)zapped_vmas,
			(unsigned long long)zapped_bytes);
	microvm_guest_ram_report_state(
		instance_vdev, EQ_MICROVM_GUEST_RAM_MMAP_STATE_REASON_UNREGISTER);
	microvm_guest_ram_put_generation_zap_targets(targets, target_count);
}

static void microvm_guest_ram_prepare_generation(eq_instance_vdev_t *instance_vdev)
{
	microvm_guest_ram_generation_zap_target_t *zap_targets = NULL;
	uint64_t generation;
	uint64_t zap_target_count = 0;
	int old_current;
	int active_mmaps;
	int current_mmaps;
	int stale_mmaps;
	int ret;

	mutex_lock(&instance_vdev->microvm_guest_ram_mmap_lock);
	old_current =
		atomic_xchg(&instance_vdev->microvm_guest_ram_current_mmap_count, 0);
	if (old_current > 0)
		atomic_add(
			old_current,
			&instance_vdev->microvm_guest_ram_stale_mmap_count);
	else if (old_current < 0)
		WARNING(
			"MicroVM instance %d guest RAM current mmap count was negative during generation advance: %d\n",
			instance_vdev->id, old_current);

	generation = instance_vdev->microvm_guest_ram_mmap_generation + 1;
	if (generation == 0)
		generation = 1;
	instance_vdev->microvm_guest_ram_mmap_generation = generation;
	microvm_guest_ram_mmap_snapshot_locked(
		instance_vdev, &generation, &active_mmaps, &current_mmaps,
		&stale_mmaps);
	mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);

	if (active_mmaps != 0 || stale_mmaps != 0)
		WARNING(
			"Creating instance %s with MicroVM guest RAM mmap generation=%llu active_mmaps=%d current_mmaps=%d stale_mmaps=%d\n",
			instance_vdev->name, (unsigned long long)generation,
			active_mmaps, current_mmaps, stale_mmaps);

	ret = microvm_guest_ram_collect_generation_zap_targets(
		instance_vdev, generation, &zap_targets, &zap_target_count);
	if (ret)
		WARNING(
			"MicroVM instance %d failed to collect stale guest RAM VMA zap targets generation=%llu ret=%d\n",
			instance_vdev->id, (unsigned long long)generation, ret);
	else
		microvm_guest_ram_zap_generation_targets(
			instance_vdev, generation, zap_targets, zap_target_count);
	microvm_guest_ram_put_generation_zap_targets(
		zap_targets, zap_target_count);
}

static void microvm_guest_ram_vma_open(struct vm_area_struct *vma)
{
	microvm_guest_ram_vma_context_t *ctx = vma->vm_private_data;

	if (!ctx)
		return;

	microvm_guest_ram_vma_context_get(ctx);
	microvm_guest_ram_mmap_account(
		ctx, "open", EQ_MICROVM_GUEST_RAM_MMAP_STATE_REASON_OPEN,
		vma->vm_start, vma->vm_end);
}

static void microvm_guest_ram_vma_close(struct vm_area_struct *vma)
{
	microvm_guest_ram_vma_context_t *ctx = vma->vm_private_data;

	if (!ctx)
		return;

	if (ctx->instance_vdev)
	{
		mutex_lock(&ctx->instance_vdev->microvm_guest_ram_mmap_lock);
		if (ctx->listed)
		{
			list_del_init(&ctx->list);
			ctx->listed = false;
		}
		mutex_unlock(&ctx->instance_vdev->microvm_guest_ram_mmap_lock);
	}
	microvm_guest_ram_mmap_unaccount(
		ctx, "close", EQ_MICROVM_GUEST_RAM_MMAP_STATE_REASON_CLOSE,
		vma->vm_start, vma->vm_end);
	vma->vm_private_data = NULL;
	microvm_guest_ram_vma_context_put(ctx);
}

static vm_fault_t microvm_guest_ram_vma_fault(struct vm_fault *vmf)
{
	struct vm_area_struct *vma = vmf->vma;
	microvm_guest_ram_vma_context_t *ctx = vma->vm_private_data;
	eq_microvm_guest_ram_translate_t *translate;
	uint64_t current_generation;
	uint64_t fault_offset;
	uint64_t fault_gpa;
	unsigned long pfn;
	int ret;

	if (!ctx || !ctx->instance_vdev || !ctx->instance_vdev->active)
		return VM_FAULT_SIGBUS;
	mutex_lock(&ctx->instance_vdev->microvm_guest_ram_mmap_lock);
	current_generation = ctx->instance_vdev->microvm_guest_ram_mmap_generation;
	mutex_unlock(&ctx->instance_vdev->microvm_guest_ram_mmap_lock);
	if (ctx->generation != current_generation)
	{
		WARNING(
			"MicroVM instance %d guest RAM stale VMA fault rejected generation=%llu current_generation=%llu gpa_start=%#llx size=%#llx addr=%#lx\n",
			ctx->instance_vdev->id,
			(unsigned long long)ctx->generation,
			(unsigned long long)current_generation,
			(unsigned long long)ctx->gpa_start,
			(unsigned long long)ctx->size, vmf->address);
		return VM_FAULT_SIGBUS;
	}
	if (vmf->address < vma->vm_start || vmf->address >= vma->vm_end)
		return VM_FAULT_SIGBUS;
	fault_offset = (uint64_t)(vmf->address - vma->vm_start);
	if (fault_offset >= ctx->size)
		return VM_FAULT_SIGBUS;
	fault_gpa = ctx->gpa_start + fault_offset;

	translate = kzalloc(sizeof(*translate), GFP_KERNEL);
	if (!translate)
		return VM_FAULT_OOM;
	translate->version = EQ_MICROVM_GUEST_RAM_TRANSLATE_VERSION;
	translate->instance_id = (uint64_t)ctx->instance_vdev->id;
	translate->gpa = fault_gpa & PAGE_MASK;
	translate->len = PAGE_SIZE;

	ret = hvc_microvm_guest_ram_translate(virt_to_phys(translate));
	if (ret < 0 || translate->result_errno != 0 || translate->hpa == 0)
	{
		WARNING(
			"MicroVM instance %d guest RAM fault translate failed gpa=%#llx ret=%d errno=%d\n",
			ctx->instance_vdev->id,
			(unsigned long long)translate->gpa, ret,
			translate->result_errno);
		kfree(translate);
		return VM_FAULT_SIGBUS;
	}

	pfn = (unsigned long)(translate->hpa >> PAGE_SHIFT);
	kfree(translate);
	return vmf_insert_pfn(vma, vmf->address & PAGE_MASK, pfn);
}

static const struct vm_operations_struct microvm_guest_ram_vm_ops = {
	.open = microvm_guest_ram_vma_open,
	.close = microvm_guest_ram_vma_close,
	.fault = microvm_guest_ram_vma_fault,
};

static eq_vfio_posted_owner_t *eq_vfio_posted_owner_find_locked(uint32_t target_vcpu)
{
	eq_vfio_posted_owner_t *owner;

	hash_for_each_possible(eq_vfio_posted_owners, owner, node, target_vcpu)
	{
		if (owner->target_vcpu == target_vcpu)
			return owner;
	}
	return NULL;
}

static void eq_irq_route_deactivate_locked(eq_irq_route_t *route, int producer_irq, const char *reason)
{
	if (!route || !route->posted_active)
		return;

	if (route->posted_owner_linked)
	{
		list_del_init(&route->posted_owner_list);
		route->posted_owner_linked = false;
	}
	irq_set_vcpu_affinity(producer_irq, NULL);
	INFO(
		"Eq IRQ bypass route idx=%u producer_irq=%d switched to software fallback (%s)\n",
		route->msix_index, producer_irq, reason);
	route->posted_active = false;
	route->posted_shared_pid = false;
	route->posted_vector_hardware = false;
	route->target_vcpu = 0;
	route->guest_vector = 0;
	route->posted_vector = 0;
	route->pi_desc_hpa = 0;
}

static void eq_vfio_posted_owner_drop_empty_locked(uint32_t target_vcpu, uint64_t owner_instance_id)
{
	eq_vfio_posted_owner_t *owner;

	owner = eq_vfio_posted_owner_find_locked(target_vcpu);
	if (!owner || owner->owner_instance_id != owner_instance_id ||
		!list_empty(&owner->active_routes))
		return;
	hash_del(&owner->node);
	kfree(owner);
}

static int eq_vfio_posted_owner_prepare_locked(eq_irq_route_t *route, uint32_t target_vcpu)
{
	eq_vfio_posted_owner_t *owner;
	eq_irq_route_t *old_route, *tmp;

	owner = eq_vfio_posted_owner_find_locked(target_vcpu);
	if (owner)
	{
		if (owner->owner_instance_id == route->instance_id)
			return 0;

		list_for_each_entry_safe(old_route, tmp, &owner->active_routes, posted_owner_list)
		{
			if (old_route->host_irq >= 0)
				eq_irq_route_deactivate_locked(
					old_route, old_route->host_irq,
					"shared VMCS PID owner replaced");
		}
		INFO(
			"Eq VFIO posted owner target_vcpu=%u owner_instance=%llu previous=%llu\n",
			target_vcpu, (unsigned long long)route->instance_id,
			(unsigned long long)owner->owner_instance_id);
		owner->owner_instance_id = route->instance_id;
		return 0;
	}

	owner = kzalloc(sizeof(*owner), GFP_KERNEL);
	if (!owner)
		return -ENOMEM;
	owner->target_vcpu = target_vcpu;
	owner->owner_instance_id = route->instance_id;
	INIT_LIST_HEAD(&owner->active_routes);
	hash_add(eq_vfio_posted_owners, &owner->node, target_vcpu);
	INFO(
		"Eq VFIO posted owner target_vcpu=%u owner_instance=%llu previous=none\n",
		target_vcpu, (unsigned long long)route->instance_id);
	return 0;
}

static void eq_vfio_posted_owner_add_route_locked(eq_irq_route_t *route, uint32_t target_vcpu)
{
	eq_vfio_posted_owner_t *owner;

	owner = eq_vfio_posted_owner_find_locked(target_vcpu);
	if (!owner || owner->owner_instance_id != route->instance_id)
		return;

	if (!route->posted_owner_linked)
	{
		list_add_tail(&route->posted_owner_list, &owner->active_routes);
		route->posted_owner_linked = true;
	}
}

static void eq_irq_route_deactivate_owned_locked(eq_irq_route_t *route, int producer_irq, const char *reason)
{
	uint32_t old_target_vcpu;

	if (!route || !route->posted_active)
		return;

	old_target_vcpu = route->target_vcpu;
	eq_irq_route_deactivate_locked(route, producer_irq, reason);
	eq_vfio_posted_owner_drop_empty_locked(old_target_vcpu, route->instance_id);
}

static bool eq_vfio_posted_owner_is_route_locked(eq_irq_route_t *route, uint32_t target_vcpu)
{
	eq_vfio_posted_owner_t *owner;

	hash_for_each_possible(eq_vfio_posted_owners, owner, node, target_vcpu)
	{
		if (owner->target_vcpu == target_vcpu &&
			owner->owner_instance_id == route->instance_id)
			return true;
	}
	return false;
}

static int eq_irq_route_try_activate(eq_irq_route_t *route, int producer_irq)
{
	eq_microvm_irq_route_query_t *query;
	phys_addr_t query_hpa;
	int ret;
	bool owner_lock_held = false;
	uint32_t old_target_vcpu = 0;
	uint32_t old_guest_vector = 0;
	uint32_t old_posted_vector = 0;
	uint64_t old_pi_desc_hpa = 0;
	bool old_owner_linked = false;
	bool old_posted_active = false;
	bool old_posted_shared_pid = false;
	bool old_posted_vector_hardware = false;
	bool query_shared_pid = false;
	bool query_requires_posted_vector = false;
	uint32_t query_posted_vector = 0;
	bool query_use_posted_vector = false;
	uint32_t pir_vector = 0;

	query = kzalloc(sizeof(*query), GFP_KERNEL);
	if (!query)
		return -ENOMEM;
	query->instance_id = route->instance_id;
	query->msix_index = route->msix_index;

	query_hpa = virt_to_phys(query);
	ret = hvc_query_microvm_irq_route(query_hpa);
	if (ret < 0)
	{
		INFO(
			"Eq IRQ bypass route idx=%u producer_irq=%d query unsupported ret=%d; keep software fallback\n",
			route->msix_index, producer_irq, ret);
		kfree(query);
		return 0;
	}
	if (query->pi_desc_hpa == 0 || query->guest_vector == 0)
	{
		if (route->posted_active)
		{
			mutex_lock(&eq_vfio_posted_owners_lock);
			eq_irq_route_deactivate_owned_locked(route, producer_irq, "PI owner unavailable");
			mutex_unlock(&eq_vfio_posted_owners_lock);
		}
		if (!route->logged_not_ready)
		{
			INFO(
				"Eq IRQ bypass route idx=%u producer_irq=%d lacks PI destination pi_desc=%#llx vector=%u; keep software fallback\n",
				route->msix_index, producer_irq,
				(unsigned long long)query->pi_desc_hpa,
				query->guest_vector);
			route->logged_not_ready = true;
		}
		kfree(query);
		return 0;
	}

#ifdef CONFIG_X86
	if (!irq_remapping_cap(IRQ_POSTING_CAP))
	{
		if (route->posted_active)
		{
			mutex_lock(&eq_vfio_posted_owners_lock);
			eq_irq_route_deactivate_owned_locked(route, producer_irq, "IRQ posting unavailable");
			mutex_unlock(&eq_vfio_posted_owners_lock);
		}
		INFO(
			"Eq IRQ bypass route idx=%u producer_irq=%d lacks IRQ posting capability; keep software fallback\n",
			route->msix_index, producer_irq);
		kfree(query);
		return 0;
	}
	mutex_lock(&eq_vfio_posted_owners_lock);
	owner_lock_held = true;
	old_target_vcpu = route->target_vcpu;
	old_guest_vector = route->guest_vector;
	old_posted_vector = route->posted_vector;
	old_pi_desc_hpa = route->pi_desc_hpa;
	old_owner_linked = route->posted_owner_linked;
	old_posted_active = route->posted_active;
	old_posted_shared_pid = route->posted_shared_pid;
	old_posted_vector_hardware = route->posted_vector_hardware;
	query_shared_pid = (query->flags & EQ_IRQ_ROUTE_FLAG_SHARED_VMCS_PID) != 0;
	query_requires_posted_vector =
		(query->flags & EQ_IRQ_ROUTE_FLAG_REQUIRES_POSTED_VECTOR) != 0;
	query_posted_vector =
		(query->flags & EQ_IRQ_ROUTE_FLAG_HAS_POSTED_VECTOR) ?
			(uint32_t)(query->reserved[0] & 0xff) :
			0;
	query_use_posted_vector =
		(eq_vfio_use_eqgate_posted_vector || query_requires_posted_vector) &&
		query_posted_vector != 0;
	if (query_requires_posted_vector && !query_use_posted_vector)
	{
		if (route->posted_active)
			eq_irq_route_deactivate_owned_locked(
				route, producer_irq, "owner-coded posted vector unavailable");
		mutex_unlock(&eq_vfio_posted_owners_lock);
		kfree(query);
		return 0;
	}

	if (route->posted_active &&
		route->target_vcpu == query->target_vcpu &&
		route->guest_vector == query->guest_vector &&
		route->posted_vector == query_posted_vector &&
		route->posted_vector_hardware == query_use_posted_vector &&
		route->pi_desc_hpa == query->pi_desc_hpa &&
		route->posted_shared_pid == query_shared_pid)
	{
		if (!query_shared_pid ||
			query_use_posted_vector ||
			eq_vfio_posted_owner_is_route_locked(route, query->target_vcpu))
		{
			route->logged_not_ready = false;
			mutex_unlock(&eq_vfio_posted_owners_lock);
			kfree(query);
			return 0;
		}
	}
	if ((query->flags & EQ_IRQ_ROUTE_FLAG_SHARED_VMCS_PID) &&
		!query_use_posted_vector)
	{
		uint32_t prepared_target_vcpu = query->target_vcpu;
		ret = eq_vfio_posted_owner_prepare_locked(route, query->target_vcpu);
		if (ret)
		{
			mutex_unlock(&eq_vfio_posted_owners_lock);
			owner_lock_held = false;
			kfree(query);
			return ret;
		}
		query->flags |= EQ_IRQ_ROUTE_FLAG_COMMIT_OWNER;
		ret = hvc_query_microvm_irq_route(query_hpa);
		if (ret < 0 || query->pi_desc_hpa == 0 || query->guest_vector == 0 ||
			query->target_vcpu != prepared_target_vcpu ||
			!(query->flags & EQ_IRQ_ROUTE_FLAG_SHARED_VMCS_PID))
		{
			eq_vfio_posted_owner_drop_empty_locked(prepared_target_vcpu, route->instance_id);
			mutex_unlock(&eq_vfio_posted_owners_lock);
			owner_lock_held = false;
			kfree(query);
			return 0;
		}
	}
	query_posted_vector =
		(query->flags & EQ_IRQ_ROUTE_FLAG_HAS_POSTED_VECTOR) ?
			(uint32_t)(query->reserved[0] & 0xff) :
			0;
	query_requires_posted_vector =
		(query->flags & EQ_IRQ_ROUTE_FLAG_REQUIRES_POSTED_VECTOR) != 0;
	query_use_posted_vector =
		(eq_vfio_use_eqgate_posted_vector || query_requires_posted_vector) &&
		query_posted_vector != 0;
	if (query_requires_posted_vector && !query_use_posted_vector)
	{
		if (route->posted_active)
			eq_irq_route_deactivate_owned_locked(
				route, producer_irq, "owner-coded posted vector unavailable");
		if (owner_lock_held)
			mutex_unlock(&eq_vfio_posted_owners_lock);
		kfree(query);
		return 0;
	}
	pir_vector = query_use_posted_vector ? query_posted_vector : query->guest_vector;
	{
		struct vcpu_data vcpu_info = {
			.pi_desc_addr = query->pi_desc_hpa,
			.vector = pir_vector,
		};
		ret = irq_set_vcpu_affinity(producer_irq, &vcpu_info);
	}
#else
	ret = -EOPNOTSUPP;
#endif
	if (ret)
	{
		eq_vfio_posted_owner_drop_empty_locked(query->target_vcpu, route->instance_id);
		if (route->posted_active)
			eq_irq_route_deactivate_owned_locked(route, producer_irq, "irq_set_vcpu_affinity failed");
		eq_vfio_posted_owner_drop_empty_locked(query->target_vcpu, route->instance_id);
		if (owner_lock_held)
			mutex_unlock(&eq_vfio_posted_owners_lock);
		INFO(
			"Eq IRQ bypass route idx=%u producer_irq=%d irq_set_vcpu_affinity failed ret=%d; keep software fallback\n",
			route->msix_index, producer_irq, ret);
		kfree(query);
		return 0;
	}

	route->target_vcpu = query->target_vcpu;
	route->guest_vector = query->guest_vector;
	route->posted_vector = query_posted_vector;
	route->pi_desc_hpa = query->pi_desc_hpa;
	route->host_irq = producer_irq;
	route->posted_active = true;
	route->posted_shared_pid = query_shared_pid;
	route->posted_vector_hardware = query_use_posted_vector;
	route->logged_not_ready = false;
	if (query_shared_pid && !query_use_posted_vector)
		eq_vfio_posted_owner_add_route_locked(route, route->target_vcpu);
	else if (route->posted_owner_linked)
	{
		list_del_init(&route->posted_owner_list);
		route->posted_owner_linked = false;
		if (old_owner_linked)
			eq_vfio_posted_owner_drop_empty_locked(old_target_vcpu, route->instance_id);
	}
	if (owner_lock_held)
		mutex_unlock(&eq_vfio_posted_owners_lock);
	INFO(
		"Eq IRQ bypass route active idx=%u host_irq=%d target_vcpu=%u pir_vector=%u posted_vector=%u guest_vector=%u use_posted_vector=%d shared_pid=%d pi_desc=%#llx old_active=%d old_target_vcpu=%u old_posted_vector=%u old_guest_vector=%u old_use_posted_vector=%d old_shared_pid=%d old_pi_desc=%#llx\n",
		route->msix_index, route->host_irq, route->target_vcpu,
		pir_vector, route->posted_vector, route->guest_vector,
		route->posted_vector_hardware, route->posted_shared_pid,
		(unsigned long long)route->pi_desc_hpa, old_posted_active,
		old_target_vcpu, old_posted_vector, old_guest_vector,
		old_posted_vector_hardware, old_posted_shared_pid,
		(unsigned long long)old_pi_desc_hpa);
	kfree(query);
	return 0;
}

static int eq_irq_route_add_producer(
	struct irq_bypass_consumer *consumer, struct irq_bypass_producer *producer)
{
	eq_irq_route_t *route =
		container_of(consumer, eq_irq_route_t, consumer);

	route->host_irq = producer->irq;
	return eq_irq_route_try_activate(route, producer->irq);
}

static void eq_irq_route_del_producer(
	struct irq_bypass_consumer *consumer, struct irq_bypass_producer *producer)
{
	eq_irq_route_t *route =
		container_of(consumer, eq_irq_route_t, consumer);

	if (route->posted_active)
	{
		mutex_lock(&eq_vfio_posted_owners_lock);
		eq_irq_route_deactivate_owned_locked(route, producer->irq, "producer disconnected");
		mutex_unlock(&eq_vfio_posted_owners_lock);
	}
	route->host_irq = -1;
	INFO(
		"Eq IRQ bypass route disconnected idx=%u producer_irq=%d\n",
		route->msix_index, producer->irq);
}

static void eq_irq_route_free(eq_irq_route_t *route)
{
	if (!route)
		return;
	mutex_lock(&eq_vfio_posted_owners_lock);
	if (route->posted_active && route->host_irq >= 0)
		eq_irq_route_deactivate_owned_locked(route, route->host_irq, "route freed");
	else if (route->posted_owner_linked)
	{
		list_del_init(&route->posted_owner_list);
		route->posted_owner_linked = false;
		eq_vfio_posted_owner_drop_empty_locked(route->target_vcpu, route->instance_id);
	}
	mutex_unlock(&eq_vfio_posted_owners_lock);
	irq_bypass_unregister_consumer(&route->consumer);
	if (route->eventfd)
		eventfd_ctx_put(route->eventfd);
	kfree(route);
}

static void eq_irq_routes_clear(eq_instance_vdev_t *vdev)
{
	eq_irq_route_t *route, *tmp;
	LIST_HEAD(routes_to_free);

	mutex_lock(&vdev->irq_routes_lock);
	list_for_each_entry_safe(route, tmp, &vdev->irq_routes, list)
	{
		list_del(&route->list);
		list_add_tail(&route->list, &routes_to_free);
	}
	mutex_unlock(&vdev->irq_routes_lock);

	list_for_each_entry_safe(route, tmp, &routes_to_free, list)
	{
		list_del(&route->list);
		eq_irq_route_free(route);
	}
}

static int eq_register_irq_route(
	eq_instance_vdev_t *instance_vdev, eq_instance_irq_route_arg_t *arg)
{
	eq_irq_route_t *route;
	struct eventfd_ctx *eventfd;
	int ret;

	if (arg->eventfd < 0)
		return -EINVAL;

	eventfd = eventfd_ctx_fdget(arg->eventfd);
	if (IS_ERR(eventfd))
		return PTR_ERR(eventfd);

	route = kzalloc(sizeof(*route), GFP_KERNEL);
	if (!route)
	{
		eventfd_ctx_put(eventfd);
		return -ENOMEM;
	}

	INIT_LIST_HEAD(&route->list);
	INIT_LIST_HEAD(&route->posted_owner_list);
	route->eventfd = eventfd;
	route->instance_id = instance_vdev->id;
	route->msix_index = arg->msix_index;
	route->host_irq = -1;
	route->consumer.token = eventfd;
	route->consumer.add_producer = eq_irq_route_add_producer;
	route->consumer.del_producer = eq_irq_route_del_producer;

	ret = irq_bypass_register_consumer(&route->consumer);
	if (ret)
	{
		INFO(
			"Eq IRQ bypass consumer register failed instance=%d msix_index=%u ret=%d; software fallback remains active\n",
			instance_vdev->id, route->msix_index, ret);
		eventfd_ctx_put(eventfd);
		kfree(route);
		return ret;
	}
	arg->flags = route->posted_active ? 0x1 : 0x0;

	mutex_lock(&instance_vdev->irq_routes_lock);
	list_add_tail(&route->list, &instance_vdev->irq_routes);
	mutex_unlock(&instance_vdev->irq_routes_lock);

	INFO(
		"Eq IRQ bypass consumer registered instance=%d msix_index=%u token=%p\n",
		instance_vdev->id, route->msix_index, route->consumer.token);
	return 0;
}

static int eq_refresh_irq_route(
	eq_instance_vdev_t *instance_vdev, eq_instance_irq_route_arg_t *arg)
{
	eq_irq_route_t *route;
	int ret = -ENOENT;

	mutex_lock(&instance_vdev->irq_routes_lock);
	list_for_each_entry(route, &instance_vdev->irq_routes, list)
	{
		if (route->msix_index != arg->msix_index)
			continue;

		ret = 0;
		if (route->host_irq >= 0)
			ret = eq_irq_route_try_activate(route, route->host_irq);
		arg->flags = route->posted_active ? 0x1 : 0x0;
		break;
	}
	mutex_unlock(&instance_vdev->irq_routes_lock);

	return ret;
}

static int eq_hyperalloc_vfio_dma_ioctl(
	eq_instance_vdev_t *instance_vdev, unsigned int cmd,
	void __user *user_arg)
{
	eq_hyperalloc_vfio_dma_op_t *op;
	phys_addr_t op_hpa;
	int ret;

	op = kzalloc(sizeof(*op), GFP_KERNEL);
	if (!op)
		return -ENOMEM;

	if (copy_from_user(op, user_arg, sizeof(*op)))
	{
		ERROR("Failed to copy HyperAlloc VFIO DMA op from user\n");
		kfree(op);
		return -EFAULT;
	}

	if (op->instance_id != 0 &&
		op->instance_id != (uint64_t)instance_vdev->id)
	{
		ERROR(
			"HyperAlloc VFIO DMA ioctl mismatched instance id: fd=%d arg=%llu\n",
			instance_vdev->id, op->instance_id);
		kfree(op);
		return -EINVAL;
	}
	op->instance_id = instance_vdev->id;
	op_hpa = virt_to_phys(op);

	if (cmd == EQ_INSTANCE_HYPERALLOC_VFIO_DMA_POLL)
		ret = hvc_hyperalloc_vfio_dma_poll(op_hpa);
	else
		ret = hvc_hyperalloc_vfio_dma_complete(op_hpa);

	if (ret < 0)
	{
		if (cmd == EQ_INSTANCE_HYPERALLOC_VFIO_DMA_POLL)
		{
			op->status = EQ_HYPERALLOC_VFIO_DMA_STATUS_NONE;
			ret = 0;
		}
		else
			op->status = EQ_HYPERALLOC_VFIO_DMA_STATUS_UNSUPPORTED;
	}

	if (copy_to_user(user_arg, op, sizeof(*op)))
	{
		ERROR("Failed to copy HyperAlloc VFIO DMA op result to user\n");
		kfree(op);
		return -EFAULT;
	}

	kfree(op);
	return ret < 0 ? ret : 0;
}

static int eq_hyperalloc_vfio_dma_debug_request_ioctl(
	eq_instance_vdev_t *instance_vdev, void __user *user_arg)
{
	eq_hyperalloc_vfio_dma_op_t *op;
	phys_addr_t op_hpa;
	int ret;

	op = kzalloc(sizeof(*op), GFP_KERNEL);
	if (!op)
		return -ENOMEM;

	if (copy_from_user(op, user_arg, sizeof(*op)))
	{
		ERROR("Failed to copy HyperAlloc VFIO DMA debug request from user\n");
		kfree(op);
		return -EFAULT;
	}

	if (op->version != EQ_HYPERALLOC_VERSION)
	{
		ERROR(
			"Invalid HyperAlloc VFIO DMA debug request version instance=%d version=%u\n",
			instance_vdev->id, op->version);
		kfree(op);
		return -EINVAL;
	}
	if (op->instance_id != 0 &&
		op->instance_id != (uint64_t)instance_vdev->id)
	{
		ERROR(
			"HyperAlloc VFIO DMA debug request mismatched instance id: fd=%d arg=%llu\n",
			instance_vdev->id, op->instance_id);
		kfree(op);
		return -EINVAL;
	}

	op->instance_id = instance_vdev->id;
	op->status = EQ_HYPERALLOC_VFIO_DMA_STATUS_PENDING;
	op->result_errno = 0;
	op_hpa = virt_to_phys(op);

	ret = hvc_hyperalloc_vfio_dma_debug_request(
		instance_vdev->id, op_hpa);
	if (ret < 0)
	{
		op->status = EQ_HYPERALLOC_VFIO_DMA_STATUS_UNSUPPORTED;
		op->result_errno = ret;
	}

	if (copy_to_user(user_arg, op, sizeof(*op)))
	{
		ERROR("Failed to copy HyperAlloc VFIO DMA debug request result to user\n");
		kfree(op);
		return -EFAULT;
	}

	kfree(op);
	return ret < 0 ? ret : 0;
}

static int eq_hyperalloc_memory_target_ioctl(
	eq_instance_vdev_t *instance_vdev, void __user *user_arg)
{
	eq_hyperalloc_pagecache_shrink_req_t *req;
	int ret;

	req = kzalloc(sizeof(*req), GFP_KERNEL);
	if (!req)
		return -ENOMEM;

	if (copy_from_user(req, user_arg, sizeof(*req)))
	{
		ERROR("Failed to copy HyperAlloc memory target request from user\n");
		kfree(req);
		return -EFAULT;
	}

	if (req->version != EQ_HYPERALLOC_VERSION)
	{
		ERROR(
			"Invalid HyperAlloc memory target request version instance=%d version=%u target_huge=%llu\n",
			instance_vdev->id, req->version,
			(unsigned long long)req->target_huge_frames);
		kfree(req);
		return -EINVAL;
	}

	ret = hvc_hyperalloc_memory_target(instance_vdev->id, virt_to_phys(req));
	if (ret < 0)
	{
		req->status = EQ_HYPERALLOC_PAGECACHE_SHRINK_STATUS_UNSUPPORTED;
		req->result_errno = ret;
	}

	if (copy_to_user(user_arg, req, sizeof(*req)))
	{
		ERROR("Failed to copy HyperAlloc memory target result to user\n");
		kfree(req);
		return -EFAULT;
	}

	kfree(req);
	return ret < 0 ? ret : 0;
}

static int eq_hyperalloc_query_ioctl(
	eq_instance_vdev_t *instance_vdev, void __user *user_arg)
{
	eq_hyperalloc_query_t *query;
	int ret;

	query = kzalloc(sizeof(*query), GFP_KERNEL);
	if (!query)
		return -ENOMEM;

	ret = hvc_hyperalloc_host_query(instance_vdev->id, virt_to_phys(query));
	if (ret < 0)
	{
		kfree(query);
		return ret;
	}

	if (copy_to_user(user_arg, query, sizeof(*query)))
	{
		ERROR("Failed to copy HyperAlloc query result to user\n");
		kfree(query);
		return -EFAULT;
	}

	kfree(query);
	return 0;
}

static int eq_hyperalloc_debug_reclaim_ioctl(
	eq_instance_vdev_t *instance_vdev, void __user *user_arg)
{
	eq_hyperalloc_debug_reclaim_req_t *req;
	int ret;

	req = kzalloc(sizeof(*req), GFP_KERNEL);
	if (!req)
		return -ENOMEM;

	if (copy_from_user(req, user_arg, sizeof(*req)))
	{
		ERROR("Failed to copy HyperAlloc debug reclaim request from user\n");
		kfree(req);
		return -EFAULT;
	}

	if (req->version != EQ_HYPERALLOC_DEBUG_RECLAIM_VERSION)
	{
		ERROR(
			"Invalid HyperAlloc debug reclaim request version instance=%d version=%u\n",
			instance_vdev->id, req->version);
		kfree(req);
		return -EINVAL;
	}
	if (req->instance_id != 0 &&
		req->instance_id != (uint64_t)instance_vdev->id)
	{
		ERROR(
			"HyperAlloc debug reclaim mismatched instance id: fd=%d arg=%llu\n",
			instance_vdev->id, req->instance_id);
		kfree(req);
		return -EINVAL;
	}

	req->instance_id = instance_vdev->id;
	req->result_errno = 0;
	ret = hvc_hyperalloc_debug_reclaim(
		instance_vdev->id, virt_to_phys(req));
	if (ret < 0)
		req->result_errno = ret;

	if (copy_to_user(user_arg, req, sizeof(*req)))
	{
		ERROR("Failed to copy HyperAlloc debug reclaim result to user\n");
		kfree(req);
		return -EFAULT;
	}

	kfree(req);
	return ret < 0 ? ret : 0;
}

static int eq_hyperalloc_eqgate_drain_ioctl(
	eq_instance_vdev_t *instance_vdev, void __user *user_arg)
{
	eq_hyperalloc_eqgate_drain_req_t *req;
	int ret;

	req = kzalloc(sizeof(*req), GFP_KERNEL);
	if (!req)
		return -ENOMEM;

	if (copy_from_user(req, user_arg, sizeof(*req)))
	{
		ERROR("Failed to copy HyperAlloc EqGate drain request from user\n");
		kfree(req);
		return -EFAULT;
	}

	if (req->version != EQ_HYPERALLOC_EQGATE_DRAIN_VERSION)
	{
		ERROR(
			"Invalid HyperAlloc EqGate drain request version instance=%d version=%u\n",
			instance_vdev->id, req->version);
		kfree(req);
		return -EINVAL;
	}
	if (req->instance_id != 0 &&
		req->instance_id != (uint64_t)instance_vdev->id)
	{
		ERROR(
			"HyperAlloc EqGate drain mismatched instance id: fd=%d arg=%llu\n",
			instance_vdev->id, req->instance_id);
		kfree(req);
		return -EINVAL;
	}

	req->instance_id = instance_vdev->id;
	req->result_errno = 0;
	ret = hvc_hyperalloc_eqgate_drain(
		instance_vdev->id, virt_to_phys(req));
	if (ret < 0)
		req->result_errno = ret;

	if (copy_to_user(user_arg, req, sizeof(*req)))
	{
		ERROR("Failed to copy HyperAlloc EqGate drain result to user\n");
		kfree(req);
		return -EFAULT;
	}

	kfree(req);
	return ret < 0 ? ret : 0;
}

static int eq_hyperalloc_eqgate_debug_enqueue_ioctl(
	eq_instance_vdev_t *instance_vdev, void __user *user_arg)
{
	eq_hyperalloc_eqgate_debug_enqueue_req_t *req;
	int ret;

	req = kzalloc(sizeof(*req), GFP_KERNEL);
	if (!req)
		return -ENOMEM;

	if (copy_from_user(req, user_arg, sizeof(*req)))
	{
		ERROR("Failed to copy HyperAlloc EqGate debug enqueue request from user\n");
		kfree(req);
		return -EFAULT;
	}

	if (req->version != EQ_HYPERALLOC_EQGATE_DEBUG_ENQUEUE_VERSION)
	{
		ERROR(
			"Invalid HyperAlloc EqGate debug enqueue request version instance=%d version=%u\n",
			instance_vdev->id, req->version);
		kfree(req);
		return -EINVAL;
	}
	if (req->instance_id != 0 &&
		req->instance_id != (uint64_t)instance_vdev->id)
	{
		ERROR(
			"HyperAlloc EqGate debug enqueue mismatched instance id: fd=%d arg=%llu\n",
			instance_vdev->id, req->instance_id);
		kfree(req);
		return -EINVAL;
	}

	req->instance_id = instance_vdev->id;
	req->result_errno = 0;
	ret = hvc_hyperalloc_eqgate_debug_enqueue(
		instance_vdev->id, virt_to_phys(req));
	if (ret < 0)
		req->result_errno = ret;

	if (copy_to_user(user_arg, req, sizeof(*req)))
	{
		ERROR("Failed to copy HyperAlloc EqGate debug enqueue result to user\n");
		kfree(req);
		return -EFAULT;
	}

	kfree(req);
	return ret < 0 ? ret : 0;
}

static int eq_microvm_guest_mem_copy_ioctl(
	eq_instance_vdev_t *instance_vdev, void __user *user_arg)
{
	eq_microvm_guest_mem_copy_t *copy;
	void *bounce;
	int ret = 0;
	bool read_from_guest;

	copy = kzalloc(sizeof(*copy), GFP_KERNEL);
	if (!copy)
		return -ENOMEM;

	if (copy_from_user(copy, user_arg, sizeof(*copy)))
	{
		ERROR("Failed to copy MicroVM guest-mem copy arg from user\n");
		kfree(copy);
		return -EFAULT;
	}

	if (copy->version != EQ_MICROVM_GUEST_MEM_COPY_VERSION ||
		copy->len == 0 || copy->len > EQ_MICROVM_GUEST_MEM_COPY_MAX_LEN ||
		!eq_guest_mem_copy_direction_valid(copy->flags) ||
		copy->user_ptr == 0)
	{
		ERROR(
			"Invalid MicroVM guest-mem copy arg instance=%d version=%u flags=%#x gpa=%#llx len=%u user_ptr=%#llx\n",
			instance_vdev->id, copy->version, copy->flags,
			(unsigned long long)copy->gpa, copy->len,
			(unsigned long long)copy->user_ptr);
		kfree(copy);
		return -EINVAL;
	}

	if (copy->instance_id != 0 &&
		copy->instance_id != (uint64_t)instance_vdev->id)
	{
		ERROR(
			"MicroVM guest-mem copy mismatched instance id: fd=%d arg=%llu\n",
			instance_vdev->id, copy->instance_id);
		kfree(copy);
		return -EINVAL;
	}

	bounce = (void *)__get_free_page(GFP_KERNEL | __GFP_ZERO);
	if (!bounce)
	{
		kfree(copy);
		return -ENOMEM;
	}

	read_from_guest =
		(copy->flags & EQ_MICROVM_GUEST_MEM_COPY_READ_FROM_GUEST) != 0;
	copy->instance_id = instance_vdev->id;
	copy->bounce_hpa = virt_to_phys(bounce);
	copy->result_errno = 0;

	if (!read_from_guest &&
		copy_from_user(
			bounce, (void __user *)(unsigned long)copy->user_ptr,
			copy->len))
	{
		ERROR("Failed to copy MicroVM guest-mem write buffer from user\n");
		ret = -EFAULT;
		goto out;
	}

	ret = hvc_microvm_guest_mem_copy(virt_to_phys(copy));
	if (ret < 0)
		goto out;

	if (copy->result_errno != 0)
	{
		ret = copy->result_errno < 0 ? copy->result_errno :
					      -copy->result_errno;
		goto out_copy_result;
	}

	if (read_from_guest &&
		copy_to_user(
			(void __user *)(unsigned long)copy->user_ptr, bounce,
			copy->len))
	{
		ERROR("Failed to copy MicroVM guest-mem read buffer to user\n");
		ret = -EFAULT;
		goto out;
	}

out_copy_result:
	if (copy_to_user(user_arg, copy, sizeof(*copy)))
	{
		ERROR("Failed to copy MicroVM guest-mem copy result to user\n");
		ret = -EFAULT;
	}
out:
	free_page((unsigned long)bounce);
	kfree(copy);
	return ret;
}

typedef struct microvm_guest_ram_zap_target
{
	microvm_guest_ram_vma_context_t *ctx;
	unsigned long va_start;
	unsigned long size;
} microvm_guest_ram_zap_target_t;

static int microvm_guest_ram_collect_zap_targets(
	eq_instance_vdev_t *instance_vdev,
	const eq_microvm_guest_ram_mmap_zap_t *zap,
	microvm_guest_ram_zap_target_t **targets_out,
	uint64_t *target_count_out)
{
	microvm_guest_ram_vma_context_t *ctx;
	microvm_guest_ram_zap_target_t *targets;
	uint64_t zap_end;
	uint64_t count = 0;
	uint64_t idx = 0;

	*targets_out = NULL;
	*target_count_out = 0;
	if (__builtin_add_overflow(zap->gpa, zap->len, &zap_end))
		return -EINVAL;

	mutex_lock(&instance_vdev->microvm_guest_ram_mmap_lock);
	list_for_each_entry(ctx, &instance_vdev->microvm_guest_ram_vma_list, list)
	{
		uint64_t ctx_end = ctx->gpa_start + ctx->size;
		if (zap->gpa < ctx_end && zap_end > ctx->gpa_start)
			count++;
	}
	if (count == 0)
	{
		mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);
		return 0;
	}
	targets = kcalloc(count, sizeof(*targets), GFP_KERNEL);
	if (!targets)
	{
		mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);
		return -ENOMEM;
	}
	list_for_each_entry(ctx, &instance_vdev->microvm_guest_ram_vma_list, list)
	{
		uint64_t ctx_end = ctx->gpa_start + ctx->size;
		uint64_t overlap_start;
		uint64_t overlap_end;
		if (!(zap->gpa < ctx_end && zap_end > ctx->gpa_start))
			continue;
		overlap_start = max(zap->gpa, ctx->gpa_start);
		overlap_end = min(zap_end, ctx_end);
		microvm_guest_ram_vma_context_get(ctx);
		targets[idx].ctx = ctx;
		targets[idx].va_start =
			ctx->vm_start + (unsigned long)(overlap_start - ctx->gpa_start);
		targets[idx].size = (unsigned long)(overlap_end - overlap_start);
		idx++;
	}
	mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);

	*targets_out = targets;
	*target_count_out = idx;
	return 0;
}

static void microvm_guest_ram_put_zap_targets(
	microvm_guest_ram_zap_target_t *targets, uint64_t target_count)
{
	uint64_t i;

	if (!targets)
		return;
	for (i = 0; i < target_count; i++)
		microvm_guest_ram_vma_context_put(targets[i].ctx);
	kfree(targets);
}

static int eq_microvm_guest_ram_mmap_zap_ioctl(
	eq_instance_vdev_t *instance_vdev, void __user *user_arg)
{
	eq_microvm_guest_ram_mmap_zap_t zap;
	microvm_guest_ram_zap_target_t *targets = NULL;
	uint64_t target_count = 0;
	uint64_t i;
	int ret;

	if (copy_from_user(&zap, user_arg, sizeof(zap)))
	{
		ERROR("Failed to copy MicroVM guest RAM mmap zap arg from user\n");
		return -EFAULT;
	}
	if (zap.version != EQ_MICROVM_GUEST_RAM_MMAP_ZAP_VERSION ||
		zap.flags != 0 || zap.len == 0 || (zap.gpa & ~PAGE_MASK) ||
		(zap.len & ~PAGE_MASK) ||
		!microvm_guest_ram_gpa_range_valid(instance_vdev, zap.gpa, zap.len))
	{
		ERROR(
			"Invalid MicroVM guest RAM mmap zap arg instance=%d version=%u flags=%#x gpa=%#llx len=%#llx\n",
			instance_vdev->id, zap.version, zap.flags,
			(unsigned long long)zap.gpa, (unsigned long long)zap.len);
		return -EINVAL;
	}
	if (zap.instance_id != 0 &&
		zap.instance_id != (uint64_t)instance_vdev->id)
	{
		ERROR(
			"MicroVM guest RAM mmap zap mismatched instance id: fd=%d arg=%llu\n",
			instance_vdev->id, zap.instance_id);
		return -EINVAL;
	}

	zap.instance_id = (uint64_t)instance_vdev->id;
	zap.result_errno = 0;
	zap.zapped_vmas = 0;
	zap.zapped_bytes = 0;

	ret = microvm_guest_ram_collect_zap_targets(
		instance_vdev, &zap, &targets, &target_count);
	if (ret)
	{
		zap.result_errno = ret;
		goto out_copy;
	}

	for (i = 0; i < target_count; i++)
	{
		microvm_guest_ram_vma_context_t *ctx = targets[i].ctx;
		struct vm_area_struct *vma;
		unsigned long va_start = targets[i].va_start;
		unsigned long va_end = va_start + targets[i].size;

		if (!ctx || !ctx->mm || targets[i].size == 0)
			continue;
		mmap_write_lock(ctx->mm);
		vma = find_vma(ctx->mm, va_start);
		if (vma && vma->vm_start <= va_start && vma->vm_end >= va_end &&
			vma->vm_private_data == ctx)
		{
			zap_vma_ptes(vma, va_start, targets[i].size);
			zap.zapped_vmas++;
			zap.zapped_bytes += targets[i].size;
		}
		else if (zap.result_errno == 0)
			zap.result_errno = -ENOENT;
		mmap_write_unlock(ctx->mm);
	}

	INFO(
		"MicroVM instance %d guest RAM mmap zap gpa[0x%llx-0x%llx] targets=%llu zapped_vmas=%llu zapped_bytes=0x%llx errno=%d\n",
		instance_vdev->id, (unsigned long long)zap.gpa,
		(unsigned long long)(zap.gpa + zap.len),
		(unsigned long long)target_count,
		(unsigned long long)zap.zapped_vmas,
		(unsigned long long)zap.zapped_bytes, zap.result_errno);
	microvm_guest_ram_report_state(
		instance_vdev, EQ_MICROVM_GUEST_RAM_MMAP_STATE_REASON_ZAP);

out_copy:
	microvm_guest_ram_put_zap_targets(targets, target_count);
	if (copy_to_user(user_arg, &zap, sizeof(zap)))
	{
		ERROR("Failed to copy MicroVM guest RAM mmap zap result to user\n");
		return -EFAULT;
	}
	return zap.result_errno < 0 ? zap.result_errno : 0;
}

static int eq_microvm_guest_ram_mmap_zap_op_ioctl(
	eq_instance_vdev_t *instance_vdev, unsigned int cmd, void __user *user_arg)
{
	eq_microvm_guest_ram_mmap_zap_op_t *op;
	int ret;

	op = kzalloc(sizeof(*op), GFP_KERNEL);
	if (!op)
		return -ENOMEM;

	if (copy_from_user(op, user_arg, sizeof(*op)))
	{
		ERROR("Failed to copy MicroVM guest RAM mmap zap op from user\n");
		kfree(op);
		return -EFAULT;
	}
	if (op->version != 0 &&
		op->version != EQ_MICROVM_GUEST_RAM_MMAP_ZAP_OP_VERSION)
	{
		ERROR(
			"Invalid MicroVM guest RAM mmap zap op version instance=%d version=%u\n",
			instance_vdev->id, op->version);
		kfree(op);
		return -EINVAL;
	}
	if (op->instance_id != 0 &&
		op->instance_id != (uint64_t)instance_vdev->id)
	{
		ERROR(
			"MicroVM guest RAM mmap zap op mismatched instance id: fd=%d arg=%llu\n",
			instance_vdev->id, op->instance_id);
		kfree(op);
		return -EINVAL;
	}

	op->instance_id = (uint64_t)instance_vdev->id;
	if (cmd == EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_ZAP_POLL)
	{
		ret = hvc_microvm_guest_ram_mmap_zap_poll(virt_to_phys(op));
	}
	else
	{
		if (op->version != EQ_MICROVM_GUEST_RAM_MMAP_ZAP_OP_VERSION)
		{
			kfree(op);
			return -EINVAL;
		}
		ret = hvc_microvm_guest_ram_mmap_zap_complete(virt_to_phys(op));
	}
	if (ret < 0)
		op->result_errno = ret;

	if (copy_to_user(user_arg, op, sizeof(*op)))
	{
		ERROR("Failed to copy MicroVM guest RAM mmap zap op result to user\n");
		kfree(op);
		return -EFAULT;
	}

	kfree(op);
	return ret < 0 ? ret : 0;
}

static int instance_dev_open(struct inode *inode, struct file *file)
{
	eq_instance_vdev_t *instance_vdev =
		container_of(file->private_data, eq_instance_vdev_t, misc);
	eq_instance_file_context_t *ctx;

	if (!instance_vdev->active)
	{
		ERROR("Instance %s is not active\n", instance_vdev->name);
		return -ENODEV;
	}
	ctx = kzalloc(sizeof(*ctx), GFP_KERNEL);
	if (!ctx)
		return -ENOMEM;
	ctx->instance_vdev = instance_vdev;
	ctx->generation = instance_vdev->microvm_guest_ram_mmap_generation;
	file->private_data = ctx;

	INFO(
		"Opened instance device %s with ID %d generation=%llu\n",
		instance_vdev->name, instance_vdev->id,
		(unsigned long long)ctx->generation);

	return 0;
}

static ssize_t instance_dev_read(
	struct file *file, char __user *buf, size_t count, loff_t *ppos)
{
	eq_instance_vdev_t *instance_vdev = active_instance_vdev_from_file(file);

	if (!instance_vdev)
	{
		ERROR("Instance fd read on inactive or stale device\n");
		return -ENODEV;
	}

	// Implement read logic here
	return 0; // Placeholder
}

static ssize_t instance_dev_write(
	struct file *file, const char __user *buf, size_t count, loff_t *ppos)
{
	eq_instance_vdev_t *instance_vdev = active_instance_vdev_from_file(file);

	if (!instance_vdev)
	{
		ERROR("Instance fd write on inactive or stale device\n");
		return -ENODEV;
	}

	// Implement write logic here
	return 0; // Placeholder
}

static int instance_dev_release(struct inode *inode, struct file *file)
{
	eq_instance_file_context_t *ctx = file->private_data;
	eq_instance_vdev_t *instance_vdev = ctx ? ctx->instance_vdev : NULL;

	if (instance_vdev)
		INFO(
			"Closing instance device %s with ID %d fd_generation=%llu current_generation=%llu active=%d\n",
			instance_vdev->name, instance_vdev->id,
			(unsigned long long)ctx->generation,
			(unsigned long long)instance_vdev->microvm_guest_ram_mmap_generation,
			instance_vdev->active);

	// Implement release logic here if needed
	kfree(ctx);
	file->private_data = NULL;
	return 0;
}

static long instance_dev_ioctl(
	struct file *file, unsigned int cmd, unsigned long arg)
{
	eq_instance_vdev_t *instance_vdev = active_instance_vdev_from_file(file);

	if (!instance_vdev)
	{
		ERROR("Instance fd ioctl on inactive or stale device\n");
		return -ENODEV;
	}

	switch (cmd)
	{
	case EQ_INSTANCE_INJECT_IRQ:
	{
		eq_instance_irq_inject_arg_t irq_arg;
		int ret;
		if (copy_from_user(
				&irq_arg, (void __user *)arg,
				sizeof(eq_instance_irq_inject_arg_t)))
		{
			ERROR("Failed to copy EQ_INSTANCE_INJECT_IRQ arg from user\n");
			return -EFAULT;
		}

		if (irq_arg.instance_id != 0 &&
			irq_arg.instance_id != (uint64_t)instance_vdev->id)
		{
			ERROR(
				"EQ_INSTANCE_INJECT_IRQ mismatched instance id: fd=%d arg=%llu\n",
				instance_vdev->id, irq_arg.instance_id);
			return -EINVAL;
		}

		ret = hvc_inject_microvm_irq(instance_vdev->id, irq_arg.msix_index);
		if (ret < 0)
		{
			ERROR(
				"HMicroVMInjectIrq failed for instance %d msix_index=%u ret=%d\n",
				instance_vdev->id, irq_arg.msix_index, ret);
			return ret;
		}
		/*
		INFO(
			"Injected MicroVM IRQ via HVC: instance=%d msix_index=%u\n",
			instance_vdev->id, irq_arg.msix_index);
		*/
		return 0;
	}
	case EQ_INSTANCE_REGISTER_IRQ_ROUTE:
	case EQ_INSTANCE_REFRESH_IRQ_ROUTE:
	{
		eq_instance_irq_route_arg_t route_arg;
		int ret;
		if (copy_from_user(
				&route_arg, (void __user *)arg,
				sizeof(eq_instance_irq_route_arg_t)))
		{
			ERROR("Failed to copy EQ_INSTANCE_*_IRQ_ROUTE arg from user\n");
			return -EFAULT;
		}

		if (route_arg.instance_id != 0 &&
		route_arg.instance_id != (uint64_t)instance_vdev->id)
		{
			ERROR(
				"EQ_INSTANCE_REGISTER_IRQ_ROUTE mismatched instance id: fd=%d arg=%llu\n",
				instance_vdev->id, route_arg.instance_id);
			return -EINVAL;
		}

		if (cmd == EQ_INSTANCE_REGISTER_IRQ_ROUTE)
			ret = eq_register_irq_route(instance_vdev, &route_arg);
		else
			ret = eq_refresh_irq_route(instance_vdev, &route_arg);
		if (ret < 0)
			return ret;
		if (copy_to_user((void __user *)arg, &route_arg, sizeof(route_arg)))
		{
			ERROR("Failed to copy EQ_INSTANCE_*_IRQ_ROUTE result back to user\n");
			return -EFAULT;
		}
		return 0;
	}
	case EQ_INSTANCE_SET_VCPU_COUNT:
	{
		eq_instance_vcpu_resize_arg_t resize_arg;
		int ret;
		if (copy_from_user(
				&resize_arg, (void __user *)arg,
				sizeof(eq_instance_vcpu_resize_arg_t)))
		{
			ERROR("Failed to copy EQ_INSTANCE_SET_VCPU_COUNT arg from user\n");
			return -EFAULT;
		}

		if (resize_arg.instance_id != 0 &&
			resize_arg.instance_id != (uint64_t)instance_vdev->id)
		{
			ERROR(
				"EQ_INSTANCE_SET_VCPU_COUNT mismatched instance id: fd=%d arg=%llu\n",
				instance_vdev->id, resize_arg.instance_id);
			return -EINVAL;
		}

		ret = hvc_set_microvm_vcpu_count(
			instance_vdev->id, resize_arg.vcpu_count);
		if (ret < 0)
		{
			ERROR(
				"HMicroVMSetVcpuCount failed for instance %d vcpu_count=%u ret=%d\n",
				instance_vdev->id, resize_arg.vcpu_count, ret);
			return ret;
		}
		INFO(
			"MicroVM instance %d desired vCPU count set to %u\n",
			instance_vdev->id, resize_arg.vcpu_count);
		return 0;
	}
	case EQ_INSTANCE_MICROVM_STOP:
	{
		eq_microvm_stop_arg_t stop_arg;
		int ret;
		if (copy_from_user(
				&stop_arg, (void __user *)arg,
				sizeof(eq_microvm_stop_arg_t)))
		{
			ERROR("Failed to copy EQ_INSTANCE_MICROVM_STOP arg from user\n");
			return -EFAULT;
		}

		if (stop_arg.instance_id != 0 &&
			stop_arg.instance_id != (uint64_t)instance_vdev->id)
		{
			ERROR(
				"EQ_INSTANCE_MICROVM_STOP mismatched instance id: fd=%d arg=%llu\n",
				instance_vdev->id, stop_arg.instance_id);
			return -EINVAL;
		}

		ret = hvc_microvm_stop(instance_vdev->id);
		if (ret < 0)
		{
			ERROR(
				"HMicroVMStop failed for instance %d ret=%d\n",
				instance_vdev->id, ret);
			return ret;
		}

		stop_arg.instance_id = instance_vdev->id;
		stop_arg.active_pcpus_signalled = (uint64_t)ret;
		if (copy_to_user((void __user *)arg, &stop_arg, sizeof(stop_arg)))
		{
			ERROR("Failed to copy EQ_INSTANCE_MICROVM_STOP result back to user\n");
			return -EFAULT;
		}
		INFO(
			"MicroVM instance %d stopped active_pcpus_signalled=%d\n",
			instance_vdev->id, ret);
		return 0;
	}
	case EQ_INSTANCE_MICROVM_BOOT:
	{
		eq_microvm_boot_arg_t boot_arg;
		int ret;
		if (copy_from_user(
				&boot_arg, (void __user *)arg,
				sizeof(eq_microvm_boot_arg_t)))
		{
			ERROR("Failed to copy EQ_INSTANCE_MICROVM_BOOT arg from user\n");
			return -EFAULT;
		}

		if (boot_arg.instance_id != 0 &&
			boot_arg.instance_id != (uint64_t)instance_vdev->id)
		{
			ERROR(
				"EQ_INSTANCE_MICROVM_BOOT mismatched instance id: fd=%d arg=%llu\n",
				instance_vdev->id, boot_arg.instance_id);
			return -EINVAL;
		}

		ret = hvc_microvm_boot(
			instance_vdev->id, boot_arg.entry_point,
			boot_arg.boot_protocol);
		if (ret < 0)
		{
			ERROR(
				"HMicroVMBoot failed for instance %d entry_point=%llx boot_protocol=%u ret=%d\n",
				instance_vdev->id, boot_arg.entry_point,
				boot_arg.boot_protocol, ret);
			return ret;
		}

		boot_arg.instance_id = instance_vdev->id;
		if (copy_to_user((void __user *)arg, &boot_arg, sizeof(boot_arg)))
		{
			ERROR("Failed to copy EQ_INSTANCE_MICROVM_BOOT result back to user\n");
			return -EFAULT;
		}
		INFO(
			"MicroVM instance %d booted entry_point=%llx boot_protocol=%u\n",
			instance_vdev->id, boot_arg.entry_point,
			boot_arg.boot_protocol);
		return 0;
	}
	case EQ_INSTANCE_HYPERALLOC_VFIO_DMA_POLL:
	case EQ_INSTANCE_HYPERALLOC_VFIO_DMA_COMPLETE:
		return eq_hyperalloc_vfio_dma_ioctl(
			instance_vdev, cmd, (void __user *)arg);
	case EQ_INSTANCE_HYPERALLOC_VFIO_DMA_DEBUG_REQUEST:
		return eq_hyperalloc_vfio_dma_debug_request_ioctl(
			instance_vdev, (void __user *)arg);
	case EQ_INSTANCE_HYPERALLOC_MEMORY_TARGET:
		return eq_hyperalloc_memory_target_ioctl(
			instance_vdev, (void __user *)arg);
	case EQ_INSTANCE_HYPERALLOC_QUERY:
		return eq_hyperalloc_query_ioctl(
			instance_vdev, (void __user *)arg);
	case EQ_INSTANCE_HYPERALLOC_DEBUG_RECLAIM:
		return eq_hyperalloc_debug_reclaim_ioctl(
			instance_vdev, (void __user *)arg);
	case EQ_INSTANCE_HYPERALLOC_EQGATE_DRAIN:
		return eq_hyperalloc_eqgate_drain_ioctl(
			instance_vdev, (void __user *)arg);
	case EQ_INSTANCE_HYPERALLOC_EQGATE_DEBUG_ENQUEUE:
		return eq_hyperalloc_eqgate_debug_enqueue_ioctl(
			instance_vdev, (void __user *)arg);
	case EQ_INSTANCE_MICROVM_GUEST_MEM_COPY:
		return eq_microvm_guest_mem_copy_ioctl(
			instance_vdev, (void __user *)arg);
	case EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_ZAP:
		return eq_microvm_guest_ram_mmap_zap_ioctl(
			instance_vdev, (void __user *)arg);
	case EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_ZAP_POLL:
	case EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_ZAP_COMPLETE:
		return eq_microvm_guest_ram_mmap_zap_op_ioctl(
			instance_vdev, cmd, (void __user *)arg);
	case EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_QUERY_V1:
	{
		eq_microvm_guest_ram_mmap_query_v1_t query;
		uint64_t generation;
		int active_mmaps;
		int current_mmaps;
		int stale_mmaps;

		if (copy_from_user(
				&query, (void __user *)arg,
				sizeof(eq_microvm_guest_ram_mmap_query_v1_t)))
		{
			ERROR(
				"Failed to copy EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_QUERY_V1 arg from user\n");
			return -EFAULT;
		}
		if (query.version != EQ_MICROVM_GUEST_RAM_MMAP_QUERY_VERSION_V1)
		{
			ERROR(
				"Invalid MicroVM guest RAM mmap v1 query version %u\n",
				query.version);
			return -EINVAL;
		}
		if (query.instance_id != 0 &&
			query.instance_id != (uint64_t)instance_vdev->id)
		{
			ERROR(
				"MicroVM guest RAM mmap v1 query mismatched instance id: fd=%d arg=%llu\n",
				instance_vdev->id, query.instance_id);
			return -EINVAL;
		}

		mutex_lock(&instance_vdev->microvm_guest_ram_mmap_lock);
		microvm_guest_ram_mmap_snapshot_locked(
			instance_vdev, &generation, &active_mmaps, &current_mmaps,
			&stale_mmaps);
		mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);
		query.flags = 0;
		query.instance_id = instance_vdev->id;
		query.active_mmaps = (uint64_t)active_mmaps;
		if (copy_to_user((void __user *)arg, &query, sizeof(query)))
		{
			ERROR(
				"Failed to copy EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_QUERY_V1 result back to user\n");
			return -EFAULT;
		}
		return 0;
	}
	case EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_QUERY:
	{
		eq_microvm_guest_ram_mmap_query_t query;
		uint64_t generation;
		int active_mmaps;
		int current_mmaps;
		int stale_mmaps;

		if (copy_from_user(
				&query, (void __user *)arg,
				sizeof(eq_microvm_guest_ram_mmap_query_t)))
		{
			ERROR(
				"Failed to copy EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_QUERY arg from user\n");
			return -EFAULT;
		}
		if (query.version != EQ_MICROVM_GUEST_RAM_MMAP_QUERY_VERSION)
		{
			ERROR(
				"Invalid MicroVM guest RAM mmap query version %u\n",
				query.version);
			return -EINVAL;
		}
		if (query.instance_id != 0 &&
			query.instance_id != (uint64_t)instance_vdev->id)
		{
			ERROR(
				"MicroVM guest RAM mmap query mismatched instance id: fd=%d arg=%llu\n",
				instance_vdev->id, query.instance_id);
			return -EINVAL;
		}

		mutex_lock(&instance_vdev->microvm_guest_ram_mmap_lock);
		microvm_guest_ram_mmap_snapshot_locked(
			instance_vdev, &generation, &active_mmaps, &current_mmaps,
			&stale_mmaps);
		mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);
		query.flags = 0;
		query.instance_id = instance_vdev->id;
		query.generation = generation;
		query.active_mmaps = (uint64_t)active_mmaps;
		query.current_mmaps = (uint64_t)current_mmaps;
		query.stale_mmaps = (uint64_t)stale_mmaps;
		if (copy_to_user((void __user *)arg, &query, sizeof(query)))
		{
			ERROR(
				"Failed to copy EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_QUERY result back to user\n");
			return -EFAULT;
		}
		return 0;
	}
	default:
		return -ENOTTY;
	}
}

/// @brief Map the shared memory region for the instance.
/// This function will be called when `axcli` calls `mmap` on the instance fd
/// ("/dev/eqinstance_<instance_id>").
static int instance_mmap(struct file *file, struct vm_area_struct *vma)
{
	eq_instance_vdev_t *instance_vdev = active_instance_vdev_from_file(file);
	microvm_guest_ram_vma_context_t *guest_ram_ctx = NULL;
	int ret = 0;
	int instance_id;
	unsigned long pfn_start, mmap_size;
	unsigned long mem_size;
	uint64_t file_offset = 0;
	uint64_t mmap_end = 0;
	uint64_t guest_ram_gpa_start = 0;
	bool microvm_guest_ram_mmap = false;

	if (!instance_vdev)
	{
		ERROR("Instance mmap on inactive or stale device\n");
		return -ENODEV;
	}
	instance_id = instance_vdev->id;

	// Check alignment and size
	if (vma->vm_start & ~PAGE_MASK || vma->vm_end & ~PAGE_MASK)
	{
		ERROR(
			"Requested mmap start 0x%lx, end 0x%lx is not page-aligned\n",
			vma->vm_start, vma->vm_end);
		return -EINVAL;
	}

	mmap_size = vma->vm_end - vma->vm_start;

	if (instance_vdev->instance_type == 0 || instance_vdev->instance_type == 1)
	{
		// Check if the offset is the SCF magic number.
		// If so, we will map the SCF queue region.
		// This is a special case for SCF queue regions.
		if (vma->vm_pgoff == MMAP_SCF_MAGIC_NUMBER)
		{
			if (mmap_size > instance_vdev->metadata.scf_region_size)
			{
				ERROR(
					"SCF queue region size 0x%llx is smaller than requested "
					"mmap "
					"size "
					"0x%lx\n",
					instance_vdev->metadata.scf_region_size, mmap_size);
				return -EINVAL;
			}
			pfn_start =
				instance_vdev->metadata.scf_region_base_gpa >> PAGE_SHIFT;
			INFO(
				"[%s] Instance [%d] SCF queue region in instance %s, "
				"va[0x%lx-0x%lx] size 0x%lx\n",
				__func__, instance_id, instance_vdev->name, vma->vm_start,
				vma->vm_end, vma->vm_end - vma->vm_start);
			INFO(
				"[%s] Instance [%d] SCF queue region in instance %s, "
				"map to gpa: [0x%llx~0x%llx] size: 0x%llx\n",
				__func__, instance_id, instance_vdev->name,
				instance_vdev->metadata.scf_region_base_gpa,
				instance_vdev->metadata.scf_region_base_gpa +
					instance_vdev->metadata.scf_region_size,
				instance_vdev->metadata.scf_region_size);
		}
		else if (vma->vm_pgoff == MMAP_PAGE_CACHE_MAGIC_NUMBER)
		{
			if (mmap_size > instance_vdev->metadata.page_cache_pool_size)
			{
				ERROR(
					"Page cache pool size 0x%llx is smaller than requested "
					"mmap "
					"size "
					"0x%lx\n",
					instance_vdev->metadata.page_cache_pool_size, mmap_size);
				return -EINVAL;
			}
			pfn_start =
				instance_vdev->metadata.page_cache_pool_base_gpa >> PAGE_SHIFT;
			INFO(
				"[%s] Instance [%d] Page cache pool in instance %s, "
				"va[0x%lx-0x%lx] size 0x%lx\n",
				__func__, instance_id, instance_vdev->name, vma->vm_start,
				vma->vm_end, vma->vm_end - vma->vm_start);
			INFO(
				"[%s] Instance [%d] Page cache pool in instance %s, "
				"map to gpa: [0x%llx~0x%llx] size: 0x%llx\n",
				__func__, instance_id, instance_vdev->name,
				instance_vdev->metadata.page_cache_pool_base_gpa,
				instance_vdev->metadata.page_cache_pool_base_gpa +
					instance_vdev->metadata.page_cache_pool_size,
				instance_vdev->metadata.page_cache_pool_size);
		}
		else
		{
			ERROR(
				"Invalid mmap offset 0x%lx for LibOS instance %s, expecting "
				"SCF magic number 0x%x or Page Cache magic number 0x%x\n",
				vma->vm_pgoff, instance_vdev->name, (int)MMAP_SCF_MAGIC_NUMBER,
				(int)MMAP_PAGE_CACHE_MAGIC_NUMBER);
			return -EINVAL;
		}
	}
	else if (instance_vdev->instance_type == 2)
	{
		if (vma->vm_pgoff == MMAP_MICROVM_CONSOLE_MAGIC_NUMBER)
		{
			if (!instance_vdev->microvm_console_ring_virt)
			{
				ERROR(
					"MicroVM console ring is not allocated for instance %s\n",
					instance_vdev->name);
				return -EINVAL;
			}
			if (mmap_size > PAGE_SIZE)
			{
				ERROR(
					"MicroVM console ring mmap size 0x%lx exceeds one page for instance %s\n",
					mmap_size, instance_vdev->name);
				return -EINVAL;
			}
			pfn_start =
				virt_to_phys(instance_vdev->microvm_console_ring_virt) >>
				PAGE_SHIFT;
			INFO(
				"[%s] Instance [%d] MicroVM console ring in instance %s, "
				"va[0x%lx-0x%lx] size 0x%lx map to gpa: 0x%llx\n",
				__func__, instance_id, instance_vdev->name, vma->vm_start,
				vma->vm_end, mmap_size,
				(unsigned long long)virt_to_phys(
					instance_vdev->microvm_console_ring_virt));
		}
		else if (vma->vm_pgoff == MMAP_MICROVM_BLOCK_NOTIFY_MAGIC_NUMBER)
		{
			if (!instance_vdev->microvm_block_notify_ring_virt)
			{
				ERROR(
					"MicroVM block notify ring is not allocated for instance %s\n",
					instance_vdev->name);
				return -EINVAL;
			}
			if (mmap_size > PAGE_SIZE)
			{
				ERROR(
					"MicroVM block notify ring mmap size 0x%lx exceeds one page for instance %s\n",
					mmap_size, instance_vdev->name);
				return -EINVAL;
			}
			pfn_start =
				virt_to_phys(instance_vdev->microvm_block_notify_ring_virt) >>
				PAGE_SHIFT;
			INFO(
				"[%s] Instance [%d] MicroVM block notify ring in instance %s, "
				"va[0x%lx-0x%lx] size 0x%lx map to gpa: 0x%llx\n",
				__func__, instance_id, instance_vdev->name, vma->vm_start,
				vma->vm_end, mmap_size,
				(unsigned long long)virt_to_phys(
					instance_vdev->microvm_block_notify_ring_virt));
		}
		else
		{
			mem_size =
				instance_vdev->metadata.init_memory_region_size_mib * 1024 * 1024;
			file_offset = (uint64_t)vma->vm_pgoff << PAGE_SHIFT;
			// For microVM instances, we only have one memory region to map.
			if (__builtin_add_overflow(
				    file_offset, (uint64_t)mmap_size, &mmap_end) ||
			    mmap_size > mem_size || mmap_end > mem_size)
			{
				ERROR(
					"MicroVM memory region size 0x%llx is smaller than requested "
					"mmap "
					"offset 0x%llx size 0x%lx\n",
					(unsigned long long)mem_size,
					(unsigned long long)file_offset, mmap_size);
				return -EINVAL;
			}
			pfn_start =
				(instance_vdev->metadata.memory_region_base_gpa >> PAGE_SHIFT) +
				vma->vm_pgoff;
			if (!microvm_guest_ram_file_offset_to_gpa(
				    instance_vdev, file_offset, mmap_size,
				    &guest_ram_gpa_start))
			{
				ERROR(
					"MicroVM memory mmap offset 0x%llx size 0x%lx cannot map to guest RAM GPA for instance %s\n",
					(unsigned long long)file_offset, mmap_size,
					instance_vdev->name);
				return -EINVAL;
			}
			microvm_guest_ram_mmap = true;
			INFO(
				"[%s] Instance [%d] MicroVM memory region in instance %s, "
				"va[0x%lx-0x%lx] size 0x%lx\n",
				__func__, instance_id, instance_vdev->name, vma->vm_start,
				vma->vm_end, vma->vm_end - vma->vm_start);
			INFO(
				"[%s] Instance [%d] MicroVM memory region in instance %s, "
				"map to gpa: [0x%llx~0x%llx] size: 0x%lx, total [0x%llx~0x%llx] "
				"size: 0x%llx\n",
				__func__, instance_id, instance_vdev->name,
				instance_vdev->metadata.memory_region_base_gpa +
					(vma->vm_pgoff << PAGE_SHIFT),
				instance_vdev->metadata.memory_region_base_gpa +
					(vma->vm_pgoff << PAGE_SHIFT) + mmap_size,
				mmap_size, instance_vdev->metadata.memory_region_base_gpa,
				instance_vdev->metadata.memory_region_base_gpa + mem_size,
				(unsigned long long)mem_size);
		}
	}
	else
	{
		ERROR(
			"Instance %s has unknown instance type %d\n", instance_vdev->name,
			instance_vdev->instance_type);
		return -EINVAL;
	}

	if (microvm_guest_ram_mmap)
	{
		guest_ram_ctx = kzalloc(sizeof(*guest_ram_ctx), GFP_KERNEL);
		if (!guest_ram_ctx)
			return -ENOMEM;
		guest_ram_ctx->instance_vdev = instance_vdev;
		guest_ram_ctx->mm = vma->vm_mm;
		mmgrab(guest_ram_ctx->mm);
		INIT_LIST_HEAD(&guest_ram_ctx->list);
		guest_ram_ctx->generation = instance_generation_from_file(file);
		guest_ram_ctx->gpa_start = guest_ram_gpa_start;
		guest_ram_ctx->size = mmap_size;
		guest_ram_ctx->vm_start = vma->vm_start;
		guest_ram_ctx->vm_end = vma->vm_end;
		guest_ram_ctx->listed = false;
		atomic_set(&guest_ram_ctx->refs, 1);
	}

	INFO(
		"[%s] Instance [%d] remap_pfn_range: va[0x%lx-0x%lx], pgoff 0x%lx\n",
		__func__, instance_id, vma->vm_start, vma->vm_end, vma->vm_pgoff);
	INFO(
		"[%s] Instance [%d] remap_pfn_range: pfn_start 0x%lx,mmap_size 0x%lx\n",
		__func__, instance_id, pfn_start, mmap_size);

	if (microvm_guest_ram_mmap)
	{
		vm_flags_set(
			vma,
			VM_IO | VM_PFNMAP | VM_DONTEXPAND | VM_DONTDUMP |
				VM_DONTCOPY);
		vma->vm_ops = &microvm_guest_ram_vm_ops;
		vma->vm_private_data = guest_ram_ctx;
		mutex_lock(&instance_vdev->microvm_guest_ram_mmap_lock);
		list_add_tail(
			&guest_ram_ctx->list,
			&instance_vdev->microvm_guest_ram_vma_list);
		guest_ram_ctx->listed = true;
		mutex_unlock(&instance_vdev->microvm_guest_ram_mmap_lock);
		microvm_guest_ram_mmap_account(
			guest_ram_ctx, "mmap",
			EQ_MICROVM_GUEST_RAM_MMAP_STATE_REASON_MMAP, vma->vm_start,
			vma->vm_end);
		INFO(
			"[%s] Instance [%d] MicroVM guest RAM lazy fault mmap gpa[0x%llx-0x%llx] va[0x%lx-0x%lx]\n",
			__func__, instance_id,
			(unsigned long long)guest_ram_ctx->gpa_start,
			(unsigned long long)(guest_ram_ctx->gpa_start + guest_ram_ctx->size),
			vma->vm_start, vma->vm_end);
		return 0;
	}

	ret = remap_pfn_range(
		vma, vma->vm_start, pfn_start, mmap_size, vma->vm_page_prot);

	if (ret)
	{
		ERROR(
			"%s: remap_pfn_range failed at [0x%lx  0x%lx]\n", __func__,
			vma->vm_start, vma->vm_end);
		kfree(guest_ram_ctx);
	}
	else if (microvm_guest_ram_mmap)
	{
		vma->vm_ops = &microvm_guest_ram_vm_ops;
		vma->vm_private_data = guest_ram_ctx;
		microvm_guest_ram_mmap_account(
			guest_ram_ctx, "mmap",
			EQ_MICROVM_GUEST_RAM_MMAP_STATE_REASON_MMAP, vma->vm_start,
			vma->vm_end);
	}

	return ret;
}

static const struct file_operations instance_fops = {
	.owner = THIS_MODULE,
	.open = instance_dev_open,
	.read = instance_dev_read,
	.write = instance_dev_write,
	.unlocked_ioctl = instance_dev_ioctl,
	.compat_ioctl = instance_dev_ioctl,
	.mmap = instance_mmap,
	.release = instance_dev_release,
};

int create_instance(eq_create_instance_arg_t *arg)
{
	int instance_id;
	int ret = 0;
	eq_instance_vdev_t *instance_vdev = NULL;

	eq_instance_metadata_t *instance_metadata;
	phys_addr_t instance_metadata_ptr_gpa;
	void *microvm_console_ring_virt = NULL;
	void *microvm_block_notify_ring_virt = NULL;

	// pid_t pid = task_pid_nr(current);
	// const char *comm = current->comm;

	instance_metadata = kmalloc(sizeof(eq_instance_metadata_t), GFP_KERNEL);
	if (!instance_metadata)
	{
		ERROR("Failed to allocate memory for instance metadata\n");
		ret = -ENOMEM;
		return ret;
	}

	memset(instance_metadata, 0, sizeof(eq_instance_metadata_t));

	if (arg->instance_type < 2)
	{

		INFO(
			"Creating LibOS instance %d with type %llu, mapping type "
			"%llu\n",
			instance_id, arg->instance_type, arg->mapping_type);
	}
	else if (arg->instance_type == 2)
	{
		// Fill in the instance metadata based on the arguments provided.
		instance_metadata->init_memory_region_size_mib = arg->init_mem_size_mib;
		instance_metadata->max_memory_region_size_mib = arg->max_mem_size_mib;
		instance_metadata->init_vcpu_num = arg->init_vcpu_num;
		instance_metadata->max_vcpu_num = arg->max_vcpu_num;
		instance_metadata->passthrough_device_count = arg->passthrough_device_count;
		for (int i = 0; i < EQ_MAX_PASSTHROUGH_DEVICES; i++)
		{
			instance_metadata->passthrough_bdf[i] = arg->passthrough_bdf[i];
		}
		instance_metadata->vfio_flags = arg->vfio_flags;
		instance_metadata->vfio_iommu_group = arg->vfio_iommu_group;
		instance_metadata->vfio_guest_visible_bdf = arg->vfio_guest_visible_bdf;
		instance_metadata->vfio_bar_count = arg->vfio_bar_count;
		for (int i = 0; i < EQ_MAX_VFIO_BARS; i++)
		{
			instance_metadata->vfio_bar_start[i] = arg->vfio_bar_start[i];
			instance_metadata->vfio_bar_size[i] = arg->vfio_bar_size[i];
			instance_metadata->vfio_bar_flags[i] = arg->vfio_bar_flags[i];
		}
		instance_metadata->vfio_pci_cfg_space_len = arg->vfio_pci_cfg_space_len;
		memcpy(
			instance_metadata->vfio_pci_cfg_space,
			arg->vfio_pci_cfg_space,
			EQ_MAX_PCI_CFG_SPACE_BYTES);
		microvm_console_ring_virt = (void *)__get_free_page(
			GFP_KERNEL | __GFP_ZERO);
		if (!microvm_console_ring_virt)
		{
			ERROR("Failed to allocate MicroVM console ring page\n");
			ret = -ENOMEM;
			goto err_free;
		}
		instance_metadata->microvm_console_ring_gpa =
			virt_to_phys(microvm_console_ring_virt);
		if (arg->microvm_block_device_count > 0)
		{
			instance_metadata->microvm_block_flags =
				arg->microvm_block_flags | EQ_MICROVM_BLOCK_FLAG_ENABLED;
			instance_metadata->microvm_block_device_count =
				arg->microvm_block_device_count;
			instance_metadata->microvm_block_capacity_sectors =
				arg->microvm_block_capacity_sectors;
			microvm_block_notify_ring_virt = (void *)__get_free_page(
				GFP_KERNEL | __GFP_ZERO);
			if (!microvm_block_notify_ring_virt)
			{
				ERROR("Failed to allocate MicroVM block notify ring page\n");
				ret = -ENOMEM;
				goto err_free;
			}
			microvm_block_notify_ring_init(microvm_block_notify_ring_virt);
			instance_metadata->microvm_block_notify_ring_gpa =
				virt_to_phys(microvm_block_notify_ring_virt);
		}
	}

	instance_metadata_ptr_gpa = virt_to_phys(instance_metadata);

	// Create a new instance through the hypervisor call.
	instance_id = hvc_create_instance(
		arg->instance_type, arg->mapping_type, instance_metadata_ptr_gpa);

	if (arg->instance_type < 2)
	{

		INFO(
			"Creating LibOS instance %d with type %llu, mapping type %llu\n"
			"gets scf queue base @ 0x%llx, size 0x%llx\n"
			"gets page cache base @ 0x%llx, size 0x%llx\n",
			instance_id, arg->instance_type, arg->mapping_type,
			instance_metadata->scf_region_base_gpa,
			instance_metadata->scf_region_size,
			instance_metadata->page_cache_pool_base_gpa,
			instance_metadata->page_cache_pool_size);
	}
	else if (arg->instance_type == 2)
	{
		INFO(
			"Creating microVM instance %d\n"
			"memory region base @ 0x%llx, init mem size %lld MB, init vcpu "
			"number %lld, passthrough devices %lld\n",
			instance_id, instance_metadata->memory_region_base_gpa,
			instance_metadata->init_memory_region_size_mib,
			instance_metadata->init_vcpu_num,
			instance_metadata->passthrough_device_count);
	}

	if (instance_id < 0)
	{
		ERROR(
			"Failed to create instance through hypervisor, error code: "
			"%d\n",
			instance_id);
		ret = instance_id;
		goto err_free;
	}

	if (instance_id >= MAX_EQ_INSTANCES_NUM)
	{
		ERROR(
			"Instance ID %d exceeds maximum allowed instances %d\n",
			instance_id, MAX_EQ_INSTANCES_NUM);
		ret = -EINVAL;
		goto err_free;
	}
	// Set the instance ID in the argument structure,
	// which will be copied back to user space.
	// This is necessary for the user space to know the assigned instance
	// ID.
	arg->instance_id = (uint64_t)instance_id;

	instance_vdev = &instances_array[arg->instance_id];

	if (instance_vdev->active)
	{
		// The instance is already active, but AxVisor still assigned this
		// instance ID, which means that the instance has been removed
		// but not yet cleaned up.
		INFO(
			"Instance %d is already active, but it was removed, reusing "
			"it\n",
			instance_id);
		// Reset the instance vdev to reuse it.
		remove_instance(instance_id);
	}

	if (instance_vdev->active)
	{
		ERROR(
			"Instance %d is already active, cannot create a new one\n",
			instance_id);
		ret = -EEXIST;
		goto err_free;
	}

	instance_vdev->id = instance_id;
	instance_vdev->instance_type = arg->instance_type;
	instance_vdev->status = STATUS_CREATED;
	instance_vdev->microvm_console_ring_virt = microvm_console_ring_virt;
	microvm_console_ring_virt = NULL;
	instance_vdev->microvm_block_notify_ring_virt =
		microvm_block_notify_ring_virt;
	microvm_block_notify_ring_virt = NULL;
	INIT_LIST_HEAD(&instance_vdev->irq_routes);
	mutex_init(&instance_vdev->irq_routes_lock);
	snprintf(
		instance_vdev->name, sizeof(instance_vdev->name), "%s%d",
		EQINSTANCE_DEV_PREFIX, instance_vdev->id);
	microvm_guest_ram_prepare_generation(instance_vdev);

	memcpy(
		&instance_vdev->metadata, instance_metadata,
		sizeof(eq_instance_metadata_t));
	arg->microvm_console_ring_gpa =
		instance_vdev->metadata.microvm_console_ring_gpa;
	arg->microvm_block_notify_ring_gpa =
		instance_vdev->metadata.microvm_block_notify_ring_gpa;
	instance_vdev->active = true;
	microvm_guest_ram_report_state(
		instance_vdev, EQ_MICROVM_GUEST_RAM_MMAP_STATE_REASON_CREATE);

	instance_vdev->misc.name = instance_vdev->name;
	instance_vdev->misc.minor = MISC_DYNAMIC_MINOR;
	instance_vdev->misc.fops = &instance_fops;

	ret = misc_register(&instance_vdev->misc);
	if (ret)
	{
		ERROR(
			"Failed to register instance device %s with ID %d, error code: "
			"%d\n",
			instance_vdev->name, instance_vdev->id, ret);
		instance_vdev->active = false;
		goto err_free;
	}

	INFO(
		"Created instance %s with ID %d\n", instance_vdev->name,
		instance_vdev->id);

	// Update the status of the instance: Created, waiting for setup.
	instance_vdev->status = STATUS_SETTING_UP;

err_free:
	if (microvm_console_ring_virt)
	{
		free_page((unsigned long)microvm_console_ring_virt);
		microvm_console_ring_virt = NULL;
	}
	if (microvm_block_notify_ring_virt)
	{
		free_page((unsigned long)microvm_block_notify_ring_virt);
		microvm_block_notify_ring_virt = NULL;
	}
	if (ret < 0 && instance_vdev && instance_vdev->microvm_console_ring_virt)
	{
		free_page((unsigned long)instance_vdev->microvm_console_ring_virt);
		instance_vdev->microvm_console_ring_virt = NULL;
	}
	if (ret < 0 && instance_vdev && instance_vdev->microvm_block_notify_ring_virt)
	{
		free_page((unsigned long)instance_vdev->microvm_block_notify_ring_virt);
		instance_vdev->microvm_block_notify_ring_virt = NULL;
	}
	kfree(instance_metadata);

	return ret;
}

int remove_instance(int instance_id)
{
	eq_instance_vdev_t *instance_vdev;
	int ret = 0;

	if (instance_id < 0 || instance_id >= MAX_EQ_INSTANCES_NUM)
	{
		ERROR("Invalid instance ID %d\n", instance_id);
		return -EINVAL;
	}

	instance_vdev = &instances_array[instance_id];

	if (!instance_vdev->active)
	{
		ERROR(
			"Instance %s with ID %d is not active\n", instance_vdev->name,
			instance_id);
		return -ENODEV;
	}

	ret = unregister_instance_dev(instance_vdev);
	if (ret < 0)
	{
		ERROR(
			"Failed to unregister instance %s with ID %d, error code: %d\n",
			instance_vdev->name, instance_id, ret);
		return ret;
	}
	return ret;
}

int unregister_instance_dev(eq_instance_vdev_t *vdev)
{
	if (!vdev)
	{
		ERROR("Invalid instance vdev pointer\n");
		return -EINVAL;
	}
	if (!vdev->active)
	{
		ERROR("Instance %s is not active\n", vdev->name);
		return -ENODEV;
	}

	misc_deregister(&vdev->misc);
	vdev->active = false;
	microvm_guest_ram_zap_unregister_vmas(vdev);
	eq_irq_routes_clear(vdev);
	if (atomic_read(&vdev->microvm_guest_ram_mmap_count) != 0)
	{
		uint64_t generation;
		int active_mmaps;
		int current_mmaps;
		int stale_mmaps;

		mutex_lock(&vdev->microvm_guest_ram_mmap_lock);
		microvm_guest_ram_mmap_snapshot_locked(
			vdev, &generation, &active_mmaps, &current_mmaps,
			&stale_mmaps);
		mutex_unlock(&vdev->microvm_guest_ram_mmap_lock);
		WARNING(
			"Unregistering instance %s with active MicroVM guest RAM mmaps generation=%llu active_mmaps=%d current_mmaps=%d stale_mmaps=%d\n",
			vdev->name, (unsigned long long)generation, active_mmaps,
			current_mmaps, stale_mmaps);
	}
	if (vdev->microvm_console_ring_virt)
	{
		free_page((unsigned long)vdev->microvm_console_ring_virt);
		vdev->microvm_console_ring_virt = NULL;
	}
	if (vdev->microvm_block_notify_ring_virt)
	{
		free_page((unsigned long)vdev->microvm_block_notify_ring_virt);
		vdev->microvm_block_notify_ring_virt = NULL;
	}
	INFO(
		"Successfully unregistered instance %s with ID %d\n", vdev->name,
		vdev->id);
	return 0;
}

void instances_init(void)
{
	int i;

	// Just clear the instances array to ensure no garbage data.
	instances_exiting = false;
	memset(instances_array, 0, sizeof(instances_array));
	for (i = 0; i < MAX_EQ_INSTANCES_NUM; i++)
	{
		mutex_init(&instances_array[i].microvm_guest_ram_mmap_lock);
		INIT_LIST_HEAD(&instances_array[i].microvm_guest_ram_vma_list);
	}
}

void instances_exit(void)
{
	instances_exiting = true;
	for (int i = 0; i < MAX_EQ_INSTANCES_NUM; i++)
	{
		if (instances_array[i].active)
		{
			// Cleanup logic for active instances if needed
			INFO(
				"Cleaning up instance %s with ID %d\n", instances_array[i].name,
				instances_array[i].id);
			unregister_instance_dev(&instances_array[i]);
		}
	}
	memset(instances_array, 0, sizeof(instances_array));
}
