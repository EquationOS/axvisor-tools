#include <linux/fs.h>
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
#include <linux/pgtable.h>
#include <linux/uaccess.h>
#include <linux/pid.h>	 // for pid_nr()
#include <linux/sched.h> // for current
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
	uint64_t pi_desc_hpa;
	int host_irq;
	bool posted_active;
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

static DEFINE_HASHTABLE(eq_vfio_posted_owners, EQ_VFIO_POSTED_OWNER_BITS);
static DEFINE_MUTEX(eq_vfio_posted_owners_lock);

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
	struct list_head irq_routes;
	struct mutex irq_routes_lock;

	eq_instance_metadata_t metadata;
} eq_instance_vdev_t;

static eq_instance_vdev_t instances_array[MAX_EQ_INSTANCES_NUM];

int unregister_instance_dev(eq_instance_vdev_t *vdev);

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
	route->target_vcpu = 0;
	route->guest_vector = 0;
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
	bool old_owner_linked = false;

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
	old_owner_linked = route->posted_owner_linked;

	if (route->posted_active &&
		route->target_vcpu == query->target_vcpu &&
		route->guest_vector == query->guest_vector &&
		route->pi_desc_hpa == query->pi_desc_hpa)
	{
		if (!(query->flags & EQ_IRQ_ROUTE_FLAG_SHARED_VMCS_PID) ||
			eq_vfio_posted_owner_is_route_locked(route, query->target_vcpu))
		{
			route->logged_not_ready = false;
			mutex_unlock(&eq_vfio_posted_owners_lock);
			kfree(query);
			return 0;
		}
	}
	if (query->flags & EQ_IRQ_ROUTE_FLAG_SHARED_VMCS_PID)
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
	{
		struct vcpu_data vcpu_info = {
			.pi_desc_addr = query->pi_desc_hpa,
			.vector = query->guest_vector,
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
	route->pi_desc_hpa = query->pi_desc_hpa;
	route->host_irq = producer_irq;
	route->posted_active = true;
	route->logged_not_ready = false;
	if (query->flags & EQ_IRQ_ROUTE_FLAG_SHARED_VMCS_PID)
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
		"Eq IRQ bypass route active idx=%u host_irq=%d target_vcpu=%u vector=%u pi_desc=%#llx\n",
		route->msix_index, route->host_irq, route->target_vcpu,
		route->guest_vector, (unsigned long long)route->pi_desc_hpa);
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

static int instance_dev_open(struct inode *inode, struct file *file)
{
	eq_instance_vdev_t *instance_vdev =
		container_of(file->private_data, eq_instance_vdev_t, misc);

	if (!instance_vdev->active)
	{
		ERROR("Instance %s is not active\n", instance_vdev->name);
		return -ENODEV;
	}
	file->private_data = instance_vdev;

	INFO(
		"Opened instance device %s with ID %d\n", instance_vdev->name,
		instance_vdev->id);

	return 0;
}

static ssize_t instance_dev_read(
	struct file *file, char __user *buf, size_t count, loff_t *ppos)
{
	eq_instance_vdev_t *instance_vdev = file->private_data;

	if (!instance_vdev->active)
	{
		ERROR("Instance %s is not active\n", instance_vdev->name);
		return -ENODEV;
	}

	// Implement read logic here
	return 0; // Placeholder
}

static ssize_t instance_dev_write(
	struct file *file, const char __user *buf, size_t count, loff_t *ppos)
{
	eq_instance_vdev_t *instance_vdev = file->private_data;

	if (!instance_vdev->active)
	{
		ERROR("Instance %s is not active\n", instance_vdev->name);
		return -ENODEV;
	}

	// Implement write logic here
	return 0; // Placeholder
}

static int instance_dev_release(struct inode *inode, struct file *file)
{
	eq_instance_vdev_t *instance_vdev = file->private_data;

	if (!instance_vdev->active)
	{
		ERROR("Instance %s is not active\n", instance_vdev->name);
		return -ENODEV;
	}

	INFO(
		"Closing instance device %s with ID %d\n", instance_vdev->name,
		instance_vdev->id);

	// Implement release logic here if needed
	file->private_data = NULL;
	return 0;
}

static long instance_dev_ioctl(
	struct file *file, unsigned int cmd, unsigned long arg)
{
	eq_instance_vdev_t *instance_vdev = file->private_data;

	if (!instance_vdev || !instance_vdev->active)
	{
		ERROR("Instance fd ioctl on inactive device\n");
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
	default:
		return -ENOTTY;
	}
}

/// @brief Map the shared memory region for the instance.
/// This function will be called when `axcli` calls `mmap` on the instance fd
/// ("/dev/eqinstance_<instance_id>").
static int instance_mmap(struct file *file, struct vm_area_struct *vma)
{
	eq_instance_vdev_t *instance_vdev = file->private_data;
	int ret = 0;
	int instance_id = instance_vdev->id;
	unsigned long pfn_start, mmap_size;
	unsigned long mem_size;

	if (!instance_vdev->active)
	{
		ERROR("Instance %s is not active\n", instance_vdev->name);
		return -ENODEV;
	}

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
		else
		{
			mem_size =
				instance_vdev->metadata.init_memory_region_size_mib * 1024 * 1024;
			// For microVM instances, we only have one memory region to map.
			if (mmap_size > mem_size)
			{
				ERROR(
					"MicroVM memory region size 0x%llx is smaller than requested "
					"mmap "
					"size "
					"0x%lx\n",
					(unsigned long long)mem_size, mmap_size);
				return -EINVAL;
			}
			pfn_start =
				(instance_vdev->metadata.memory_region_base_gpa >> PAGE_SHIFT) +
				vma->vm_pgoff;
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

	INFO(
		"[%s] Instance [%d] remap_pfn_range: va[0x%lx-0x%lx], pgoff 0x%lx\n",
		__func__, instance_id, vma->vm_start, vma->vm_end, vma->vm_pgoff);
	INFO(
		"[%s] Instance [%d] remap_pfn_range: pfn_start 0x%lx,mmap_size 0x%lx\n",
		__func__, instance_id, pfn_start, mmap_size);

	ret = remap_pfn_range(
		vma, vma->vm_start, pfn_start, mmap_size, vma->vm_page_prot);

	if (ret)
		ERROR(
			"%s: remap_pfn_range failed at [0x%lx  0x%lx]\n", __func__,
			vma->vm_start, vma->vm_end);

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
	instance_vdev->active = true;
	instance_vdev->status = STATUS_CREATED;
	instance_vdev->microvm_console_ring_virt = microvm_console_ring_virt;
	microvm_console_ring_virt = NULL;
	INIT_LIST_HEAD(&instance_vdev->irq_routes);
	mutex_init(&instance_vdev->irq_routes_lock);

	memcpy(
		&instance_vdev->metadata, instance_metadata,
		sizeof(eq_instance_metadata_t));
	arg->microvm_console_ring_gpa =
		instance_vdev->metadata.microvm_console_ring_gpa;

	snprintf(
		instance_vdev->name, sizeof(instance_vdev->name), "%s%d",
		EQINSTANCE_DEV_PREFIX, instance_vdev->id);

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
	if (ret < 0 && instance_vdev && instance_vdev->microvm_console_ring_virt)
	{
		free_page((unsigned long)instance_vdev->microvm_console_ring_virt);
		instance_vdev->microvm_console_ring_virt = NULL;
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
	eq_irq_routes_clear(vdev);
	if (vdev->microvm_console_ring_virt)
	{
		free_page((unsigned long)vdev->microvm_console_ring_virt);
		vdev->microvm_console_ring_virt = NULL;
	}
	vdev->active = false;
	INFO(
		"Successfully unregistered instance %s with ID %d\n", vdev->name,
		vdev->id);
	return 0;
}

void instances_init(void)
{
	// Just clear the instances array to ensure no garbage data.
	memset(instances_array, 0, sizeof(instances_array));
}

void instances_exit(void)
{
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
