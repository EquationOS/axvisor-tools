#pragma once

#ifndef __KERNEL__
#include <stdint.h>
#include <sys/ioctl.h>
#endif

#define EQMANAGER_NAME "eqmanager"

#define EQINSTANCE_DEV_PREFIX "eqinstance_"

#define MAX_EQ_INSTANCES_NUM (64)

#define MMAP_SCF_MAGIC_NUMBER (0x45534346) // "ESCF"
#define MMAP_PAGE_CACHE_MAGIC_NUMBER (0x45504350) // "EPCP"
#define MMAP_MICROVM_CONSOLE_MAGIC_NUMBER (0x45564343) // "EVCC"
#define MMAP_MICROVM_BLOCK_NOTIFY_MAGIC_NUMBER (0x4556424e) // "EVBN"
#define EQ_MAX_PASSTHROUGH_DEVICES (8)
#define EQ_MAX_VFIO_BARS (6)
#define EQ_MAX_PCI_CFG_SPACE_BYTES (256)
#define EQ_MICROVM_GUEST_MEM_COPY_VERSION (1U)
#define EQ_MICROVM_GUEST_MEM_COPY_MAX_LEN (4096U)
#define EQ_MICROVM_GUEST_MEM_COPY_READ_FROM_GUEST (1U << 0)
#define EQ_MICROVM_GUEST_MEM_COPY_WRITE_TO_GUEST (1U << 1)
#define EQ_MICROVM_GUEST_RAM_MMAP_QUERY_VERSION_V1 (1U)
#define EQ_MICROVM_GUEST_RAM_MMAP_QUERY_VERSION (2U)
#define EQ_MICROVM_GUEST_RAM_MMAP_STATE_UPDATE_VERSION (1U)
#define EQ_MICROVM_GUEST_RAM_MMAP_STATE_REASON_CREATE (1U)
#define EQ_MICROVM_GUEST_RAM_MMAP_STATE_REASON_MMAP (2U)
#define EQ_MICROVM_GUEST_RAM_MMAP_STATE_REASON_OPEN (3U)
#define EQ_MICROVM_GUEST_RAM_MMAP_STATE_REASON_CLOSE (4U)
#define EQ_MICROVM_GUEST_RAM_MMAP_STATE_REASON_UNREGISTER (5U)
#define EQ_MICROVM_GUEST_RAM_MMAP_STATE_REASON_ZAP (6U)
#define EQ_MICROVM_GUEST_RAM_TRANSLATE_VERSION (1U)
#define EQ_MICROVM_GUEST_RAM_MMAP_ZAP_VERSION (1U)
#define EQ_MICROVM_GUEST_RAM_MMAP_ZAP_OP_VERSION (1U)
#define EQ_MICROVM_GUEST_RAM_MMAP_ZAP_STATUS_NONE \
	EQ_HYPERALLOC_VFIO_DMA_STATUS_NONE
#define EQ_MICROVM_GUEST_RAM_MMAP_ZAP_STATUS_PENDING \
	EQ_HYPERALLOC_VFIO_DMA_STATUS_PENDING
#define EQ_MICROVM_GUEST_RAM_MMAP_ZAP_STATUS_SUCCESS \
	EQ_HYPERALLOC_VFIO_DMA_STATUS_SUCCESS
#define EQ_MICROVM_GUEST_RAM_MMAP_ZAP_STATUS_FAILED \
	EQ_HYPERALLOC_VFIO_DMA_STATUS_FAILED
#define EQ_MICROVM_GUEST_RAM_MMAP_ZAP_STATUS_UNSUPPORTED \
	EQ_HYPERALLOC_VFIO_DMA_STATUS_UNSUPPORTED
#define EQ_HYPERALLOC_VERSION (1U)
#define EQ_HYPERALLOC_DEBUG_RECLAIM_VERSION (1U)
#define EQ_HYPERALLOC_DEBUG_RECLAIM_FLAG_GUEST_RAM_PREZAPPED (1U << 0)
#define EQ_HYPERALLOC_EQGATE_DRAIN_VERSION (1U)
#define EQ_HYPERALLOC_EQGATE_DEBUG_ENQUEUE_VERSION (1U)
#define EQ_HYPERALLOC_EQGATE_DRAIN_FLAG_EXECUTE (1U << 0)
#define EQ_HYPERALLOC_QUERY_FLAG_RUNTIME_ENABLED (1U << 0)
#define EQ_HYPERALLOC_QUERY_FLAG_VFIO_PRESENT (1U << 1)
#define EQ_HYPERALLOC_QUERY_FLAG_VFIO_DMA_DYNAMIC_SUPPORTED (1U << 2)
#define EQ_HYPERALLOC_QUERY_FLAG_VFIO_DMA_DYNAMIC_ENABLED (1U << 3)
#define EQ_HYPERALLOC_QUERY_FLAG_VFIO_DMA_BLOCKED_STALE_VMA (1U << 4)
#define EQ_HYPERALLOC_QUERY_FLAG_VFIO_PHYSICAL_RECLAIM_BLOCKED (1U << 5)
#define EQ_HYPERALLOC_QUERY_FLAG_VFIO_GUEST_RAM_MMAP_ACTIVE (1U << 6)
#define EQ_HYPERALLOC_QUERY_FLAG_VFIO_GUEST_RAM_MMAP_STALE (1U << 7)
#define EQ_HYPERALLOC_QUERY_FLAG_PHYSICAL_RELEASE_ALLOWED (1U << 8)
#define EQ_HYPERALLOC_QUERY_FLAG_PERSISTENT_HOST_RAM_CONSUMERS (1U << 9)
#define EQ_HYPERALLOC_QUERY_FLAG_EQGATE_BATCH_AVAILABLE (1U << 10)
#define EQ_HYPERALLOC_PAGECACHE_SHRINK_STATUS_NONE (0U)
#define EQ_HYPERALLOC_PAGECACHE_SHRINK_STATUS_PENDING (1U)
#define EQ_HYPERALLOC_PAGECACHE_SHRINK_STATUS_SUCCESS (2U)
#define EQ_HYPERALLOC_PAGECACHE_SHRINK_STATUS_FAILED (3U)
#define EQ_HYPERALLOC_PAGECACHE_SHRINK_STATUS_UNSUPPORTED (4U)

typedef struct eq_create_instance_arg
{
	uint64_t instance_id;	// Instance ID, set by kernel driver.
	uint64_t instance_type; // Instance type
	uint64_t mapping_type;	// Mapping type, e.g., course-grained mapping or
							// one-to-one mapping
	uint64_t init_vcpu_num; // Number of vCPUs for the instance
	uint64_t max_vcpu_num; // Number of vCPUs for the instance
	uint64_t init_mem_size_mib; // Memory size for the instance
	uint64_t max_mem_size_mib; // Max memory size for the instance
	uint64_t passthrough_device_count; // Number of passthrough BDF entries used
	uint64_t passthrough_bdf[EQ_MAX_PASSTHROUGH_DEVICES]; // Encoded as 0xddddbbddf
	uint64_t vfio_flags; // bit0: enabled, bit1: iova-gpa-identity, bit2: physical-release
	uint64_t vfio_iommu_group; // Host IOMMU group ID
	uint64_t vfio_guest_visible_bdf; // Encoded guest-visible BDF as 0xddddbbddf
	uint64_t vfio_bar_count; // Number of valid BAR entries
	uint64_t vfio_bar_start[EQ_MAX_VFIO_BARS];
	uint64_t vfio_bar_size[EQ_MAX_VFIO_BARS];
	uint64_t vfio_bar_flags[EQ_MAX_VFIO_BARS];
	// Snapshot of host config space read by axcli from a stable host path.
	// This avoids depending on host CF8/CFC at runtime inside the hypervisor.
	uint64_t vfio_pci_cfg_space_len; // Valid bytes in vfio_pci_cfg_space (<= 256)
	uint8_t vfio_pci_cfg_space[EQ_MAX_PCI_CFG_SPACE_BYTES];
	uint64_t microvm_console_ring_gpa;
	uint64_t microvm_block_flags; // bit0: split virtio-blk backend enabled
	uint64_t microvm_block_device_count;
	uint64_t microvm_block_notify_ring_gpa;
	uint64_t microvm_block_capacity_sectors;
} eq_create_instance_arg_t;

typedef struct eq_remove_instance_arg
{
	uint64_t instance_id; // Instance ID
} eq_remove_instance_arg_t;

typedef struct eq_microvm_boot_arg
{
	uint64_t instance_id; // Target instance ID, 0 means current instance fd owner
	uint64_t entry_point; // Guest kernel entry point
	uint32_t boot_protocol; // Linux boot protocol selector
	uint32_t flags; // Reserved for future boot modes
	uint64_t reserved[2];
} eq_microvm_boot_arg_t;

typedef struct eq_instance_irq_inject_arg
{
	uint64_t instance_id; // Target instance ID, 0 means current instance fd owner
	uint32_t msix_index;  // MSI-X entry index raised on host VFIO side
	uint32_t reserved;    // Reserved for alignment/future flags
} eq_instance_irq_inject_arg_t;

typedef struct eq_instance_irq_route_arg
{
	uint64_t instance_id; // Target instance ID, 0 means current instance fd owner
	int32_t eventfd;      // VFIO MSI-X eventfd token fd from axcli
	uint32_t msix_index;  // MSI-X entry index in the guest-visible table
	uint32_t flags;       // In/out flags. bit0: posted interrupt active
	uint32_t reserved;    // Reserved for alignment
} eq_instance_irq_route_arg_t;

typedef struct eq_instance_vcpu_resize_arg
{
	uint64_t instance_id; // Target instance ID, 0 means current instance fd owner
	uint32_t vcpu_count;  // Desired online vCPU count
	uint32_t flags;       // Reserved for future policies
	uint64_t reserved[2];
} eq_instance_vcpu_resize_arg_t;

typedef struct eq_microvm_stop_arg
{
	uint64_t instance_id; // Target instance ID, 0 means current instance fd owner
	uint64_t active_pcpus_signalled; // Out: pCPUs signalled by EqVisor stop
	uint64_t flags; // Reserved for future stop modes
	uint64_t reserved[2];
} eq_microvm_stop_arg_t;

typedef struct eq_hyperalloc_vfio_dma_op
{
	uint32_t version;
	uint32_t op;
	uint32_t status;
	uint32_t flags;
	uint64_t sequence;
	uint64_t instance_id;
	uint64_t frame_gpa;
	uint64_t frame_hpa;
	uint64_t iova;
	uint64_t size;
	int32_t result_errno;
	uint32_t reserved0;
	uint64_t reserved[2];
} eq_hyperalloc_vfio_dma_op_t;

typedef struct eq_hyperalloc_pagecache_shrink_req
{
	uint32_t version;
	uint32_t flags;
	uint64_t sequence;
	uint64_t target_huge_frames;
	uint64_t target_pages;
	uint64_t timeout_ms;
	uint64_t reclaimed_huge_frames;
	uint64_t remaining_file_huge_frames;
	uint32_t status;
	int32_t result_errno;
	uint64_t reserved[2];
} eq_hyperalloc_pagecache_shrink_req_t;

typedef struct eq_hyperalloc_query
{
	uint32_t version;
	uint32_t flags;
	uint32_t page_shift;
	uint32_t huge_order;
	uint32_t zone_count;
	uint32_t reserved0;
	uint64_t frame_count;
	uint64_t installed_frames;
	uint64_t soft_reclaimed_frames;
	uint64_t hard_reclaimed_frames;
	uint64_t installing_frames;
	uint64_t reclaiming_frames;
	uint64_t registered_frames;
	uint64_t pagecache_drop_notifications;
	uint64_t last_dropped_huge_frames;
	uint64_t last_file_huge_frames;
	uint64_t first_tracked_frame_gpa;
	uint64_t first_tracked_frame_hpa;
	uint32_t first_tracked_frame_state;
	uint32_t reserved1;
	uint64_t last_reclaimed_frame_gpa;
	uint64_t last_reclaimed_frame_hpa;
	uint32_t last_reclaimed_zone_id;
	uint32_t last_reclaimed_result;
	uint64_t last_installed_frame_gpa;
	uint64_t last_installed_frame_hpa;
	uint32_t last_installed_zone_id;
	uint32_t last_installed_result;
	uint64_t logical_hard_reclaims;
	uint64_t logical_returns;
	uint64_t logical_installs;
	uint64_t logical_hard_reclaim_attempts;
	uint64_t logical_hard_reclaim_failures;
	uint64_t logical_install_attempts;
	uint64_t logical_install_failures;
	uint64_t last_logical_reclaim_us;
	uint64_t max_logical_reclaim_us;
	uint64_t total_logical_reclaim_us;
	uint64_t last_logical_install_us;
	uint64_t max_logical_install_us;
	uint64_t total_logical_install_us;
	uint64_t physical_releases;
	uint64_t physical_allocations;
	uint64_t physically_released_frames;
	uint64_t last_physical_release_hpa;
	uint64_t last_physical_allocation_hpa;
	uint64_t vfio_dma_pending_requests;
	uint64_t vfio_dma_completed_requests;
	uint64_t vfio_dma_failed_requests;
	uint64_t last_vfio_dma_seq;
	uint32_t last_vfio_dma_op;
	uint32_t last_vfio_dma_status;
	uint64_t last_vfio_dma_iova;
	uint64_t last_vfio_dma_hpa;
	uint64_t last_vfio_dma_size;
	int32_t last_vfio_dma_errno;
	uint32_t vfio_dma_outstanding;
	uint64_t pagecache_shrink_pending_requests;
	uint64_t pagecache_shrink_completed_requests;
	uint64_t pagecache_shrink_failed_requests;
	uint64_t last_pagecache_shrink_seq;
	uint64_t last_pagecache_shrink_target_huge_frames;
	uint64_t last_pagecache_shrink_target_pages;
	uint64_t last_pagecache_shrink_reclaimed_huge_frames;
	uint64_t last_pagecache_shrink_remaining_file_huge_frames;
	uint32_t last_pagecache_shrink_status;
	int32_t last_pagecache_shrink_errno;
	uint64_t vfio_guest_ram_mmap_generation;
	uint64_t vfio_guest_ram_mmap_active;
	uint64_t vfio_guest_ram_mmap_current;
	uint64_t vfio_guest_ram_mmap_stale;
	uint64_t vfio_guest_ram_mmap_update_seq;
	uint32_t vfio_guest_ram_mmap_last_reason;
	uint32_t reserved3;
	uint64_t guest_ram_mmap_zap_pending_requests;
	uint64_t guest_ram_mmap_zap_completed_requests;
	uint64_t guest_ram_mmap_zap_failed_requests;
	uint64_t last_guest_ram_mmap_zap_seq;
	uint64_t last_guest_ram_mmap_zap_gpa;
	uint64_t last_guest_ram_mmap_zap_len;
	uint64_t last_guest_ram_mmap_zap_zapped_vmas;
	uint64_t last_guest_ram_mmap_zap_zapped_bytes;
	uint32_t last_guest_ram_mmap_zap_status;
	int32_t last_guest_ram_mmap_zap_errno;
	uint32_t guest_ram_mmap_zap_outstanding;
	uint32_t reserved4;
	uint32_t vfio_physical_reclaim_block_reason;
	uint32_t reserved5;
	uint64_t eqgate_hyperalloc_pcpu_count;
	uint64_t eqgate_hyperalloc_queue_capacity;
	uint64_t eqgate_hyperalloc_pending;
	uint64_t eqgate_hyperalloc_submitted;
	uint64_t eqgate_hyperalloc_drained;
	uint64_t eqgate_hyperalloc_dropped;
	uint64_t eqgate_hyperalloc_last_sequence;
	uint64_t reclaim_dma_rollback_attempts;
	uint64_t reclaim_dma_rollback_successes;
	uint64_t reclaim_dma_rollback_failures;
	uint64_t install_ept_rollback_attempts;
	uint64_t install_ept_rollback_successes;
	uint64_t install_ept_rollback_failures;
} eq_hyperalloc_query_t;

_Static_assert(sizeof(eq_hyperalloc_query_t) == 704,
	       "eq_hyperalloc_query_t must match EqHyperAllocQuery");

typedef struct eq_hyperalloc_debug_reclaim_req
{
	uint32_t version;
	uint32_t flags;
	uint64_t instance_id;
	uint32_t zone_id;
	int32_t result_errno;
	uint64_t frame_gpa;
	uint64_t frame_len;
	uint64_t installed_after;
	uint64_t soft_after;
	uint64_t hard_after;
	uint64_t physical_releases_after;
	uint64_t physically_released_after;
	uint64_t last_physical_release_hpa;
	uint64_t reserved[3];
} eq_hyperalloc_debug_reclaim_req_t;

_Static_assert(sizeof(eq_hyperalloc_debug_reclaim_req_t) == 112,
	       "eq_hyperalloc_debug_reclaim_req_t must match EqHyperAllocDebugReclaimReq");

typedef struct eq_hyperalloc_eqgate_drain_req
{
	uint32_t version;
	uint32_t flags;
	uint64_t instance_id;
	uint32_t max_requests;
	int32_t result_errno;
	uint64_t visited_pcpus;
	uint64_t pending_before;
	uint64_t drained;
	uint64_t installed;
	uint64_t unsupported;
	uint64_t failed;
	uint64_t pending_after;
	uint64_t last_sequence;
	uint64_t skipped;
	uint64_t blocked_by_other_instance;
} eq_hyperalloc_eqgate_drain_req_t;

_Static_assert(sizeof(eq_hyperalloc_eqgate_drain_req_t) == 104,
	       "eq_hyperalloc_eqgate_drain_req_t must match EqHyperAllocEqGateDrainReq");

typedef struct eq_hyperalloc_eqgate_debug_enqueue_req
{
	uint32_t version;
	uint32_t flags;
	uint64_t instance_id;
	uint32_t vcpu_id;
	uint32_t zone_id;
	int32_t result_errno;
	uint32_t reserved0;
	uint64_t frame_gpa;
	uint64_t frame_len;
	uint64_t entry_flags;
	uint64_t target_pcpu;
	uint64_t sequence;
	uint64_t pending_before;
	uint64_t pending_after;
	uint64_t submitted_after;
	uint64_t dropped_after;
	uint64_t reserved[2];
} eq_hyperalloc_eqgate_debug_enqueue_req_t;

_Static_assert(sizeof(eq_hyperalloc_eqgate_debug_enqueue_req_t) == 120,
	       "eq_hyperalloc_eqgate_debug_enqueue_req_t must match EqHyperAllocEqGateDebugEnqueueReq");

typedef struct eq_microvm_guest_mem_copy
{
	uint32_t version;
	uint32_t flags;
	uint64_t instance_id;
	uint64_t gpa;
	uint32_t len;
	int32_t result_errno;
	uint64_t user_ptr;
	uint64_t bounce_hpa;
	uint64_t reserved[4];
} eq_microvm_guest_mem_copy_t;

typedef struct eq_microvm_guest_ram_mmap_query_v1
{
	uint32_t version;
	uint32_t flags;
	uint64_t instance_id;
	uint64_t active_mmaps;
	uint64_t reserved[4];
} eq_microvm_guest_ram_mmap_query_v1_t;

typedef struct eq_microvm_guest_ram_mmap_query
{
	uint32_t version;
	uint32_t flags;
	uint64_t instance_id;
	uint64_t generation;
	uint64_t active_mmaps;
	uint64_t current_mmaps;
	uint64_t stale_mmaps;
	uint64_t reserved[3];
} eq_microvm_guest_ram_mmap_query_t;

typedef struct eq_microvm_guest_ram_mmap_state_update
{
	uint32_t version;
	uint32_t reason;
	uint64_t sequence;
	uint64_t instance_id;
	uint64_t generation;
	uint64_t active_mmaps;
	uint64_t current_mmaps;
	uint64_t stale_mmaps;
	uint64_t reserved[4];
} eq_microvm_guest_ram_mmap_state_update_t;

typedef struct eq_microvm_guest_ram_translate
{
	uint32_t version;
	uint32_t flags;
	uint64_t instance_id;
	uint64_t gpa;
	uint64_t len;
	uint64_t hpa;
	uint64_t page_size;
	int32_t result_errno;
	uint32_t reserved0;
	uint64_t reserved[3];
} eq_microvm_guest_ram_translate_t;

_Static_assert(sizeof(eq_microvm_guest_ram_translate_t) == 80,
	       "eq_microvm_guest_ram_translate_t must match EqMicroVmGuestRamTranslate");

typedef struct eq_microvm_guest_ram_mmap_zap
{
	uint32_t version;
	uint32_t flags;
	uint64_t instance_id;
	uint64_t gpa;
	uint64_t len;
	uint64_t zapped_vmas;
	uint64_t zapped_bytes;
	int32_t result_errno;
	uint32_t reserved0;
	uint64_t reserved[3];
} eq_microvm_guest_ram_mmap_zap_t;

_Static_assert(sizeof(eq_microvm_guest_ram_mmap_zap_t) == 80,
	       "eq_microvm_guest_ram_mmap_zap_t must match EqMicroVmGuestRamMmapZap");

typedef struct eq_microvm_guest_ram_mmap_zap_op
{
	uint32_t version;
	uint32_t status;
	uint64_t sequence;
	uint64_t instance_id;
	uint64_t gpa;
	uint64_t len;
	uint64_t zapped_vmas;
	uint64_t zapped_bytes;
	int32_t result_errno;
	uint32_t reserved0;
	uint64_t reserved[2];
} eq_microvm_guest_ram_mmap_zap_op_t;

_Static_assert(sizeof(eq_microvm_guest_ram_mmap_zap_op_t) == 80,
	       "eq_microvm_guest_ram_mmap_zap_op_t must match EqMicroVmGuestRamMmapZapOp");

typedef struct eq_microvm_irq_route_query
{
	uint64_t instance_id;
	uint32_t msix_index;
	uint32_t flags;
	uint32_t target_vcpu;
	uint32_t guest_vector;
	uint64_t pi_desc_hpa;
	/* reserved[0]: owner-coded posted vector when HAS_POSTED_VECTOR is set. */
	uint64_t reserved[4];
} eq_microvm_irq_route_query_t;

// int shmget(key_t key, size_t size, int shmflg);
// returns the shared memory ID (shmid) on success, or -1 on failure.
typedef struct eq_shmget_arg
{
	uint64_t key;	// Shared memory Key, passed by user space.
	uint64_t size; // Size of the shared memory region, set by user space.
	uint64_t shmflg; // Shared memory flags, e.g., IPC_CREAT, IPC_EXCL, etc.
} eq_shmget_arg_t;

// These two are performed by mmap/munmap on the fd of "/dev/eqmanager"
// void *shmat(int shmid, const void *shmaddr, int shmflg);
// int shmdt(const void *shmaddr);

// int shmctl(int shmid, int cmd, struct shmid_ds *buf);
typedef struct eq_shmctl_arg
{
	uint64_t shmid; // Shared memory ID, set by kernel driver.
	uint64_t cmd;   // Command, e.g., IPC_RMID to remove the shared memory.
	uint64_t buf;   // Pointer to a structure containing additional information.
} eq_shmctl_arg_t;


#define EQ_CREATE_INSTANCE _IOW(0, 0, eq_create_instance_arg_t)
#define EQ_REMOVE_INSTANCE _IOW(0, 1, eq_remove_instance_arg_t)
#define EQ_INSTANCE_INJECT_IRQ _IOW(0, 2, eq_instance_irq_inject_arg_t)
#define EQ_INSTANCE_REGISTER_IRQ_ROUTE _IOW(0, 3, eq_instance_irq_route_arg_t)
#define EQ_INSTANCE_REFRESH_IRQ_ROUTE _IOW(0, 4, eq_instance_irq_route_arg_t)
#define EQ_INSTANCE_SET_VCPU_COUNT _IOW(0, 5, eq_instance_vcpu_resize_arg_t)
#define EQ_INSTANCE_HYPERALLOC_VFIO_DMA_POLL \
	_IOW(0, 6, eq_hyperalloc_vfio_dma_op_t)
#define EQ_INSTANCE_HYPERALLOC_VFIO_DMA_COMPLETE \
	_IOW(0, 7, eq_hyperalloc_vfio_dma_op_t)
#define EQ_INSTANCE_MICROVM_GUEST_MEM_COPY \
	_IOW(0, 8, eq_microvm_guest_mem_copy_t)
#define EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_QUERY_V1 \
	_IOW(0, 9, eq_microvm_guest_ram_mmap_query_v1_t)
#define EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_QUERY \
	_IOW(0, 9, eq_microvm_guest_ram_mmap_query_t)
#define EQ_INSTANCE_HYPERALLOC_MEMORY_TARGET \
	_IOW(0, 10, eq_hyperalloc_pagecache_shrink_req_t)
#define EQ_INSTANCE_HYPERALLOC_QUERY \
	_IOW(0, 11, eq_hyperalloc_query_t)
#define EQ_INSTANCE_HYPERALLOC_VFIO_DMA_DEBUG_REQUEST \
	_IOW(0, 12, eq_hyperalloc_vfio_dma_op_t)
#define EQ_INSTANCE_HYPERALLOC_EQGATE_DRAIN \
	_IOW(0, 13, eq_hyperalloc_eqgate_drain_req_t)
#define EQ_INSTANCE_HYPERALLOC_EQGATE_DEBUG_ENQUEUE \
	_IOW(0, 14, eq_hyperalloc_eqgate_debug_enqueue_req_t)
#define EQ_INSTANCE_HYPERALLOC_DEBUG_RECLAIM \
	_IOW(0, 15, eq_hyperalloc_debug_reclaim_req_t)
#define EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_ZAP \
	_IOW(0, 16, eq_microvm_guest_ram_mmap_zap_t)
#define EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_ZAP_POLL \
	_IOW(0, 17, eq_microvm_guest_ram_mmap_zap_op_t)
#define EQ_INSTANCE_MICROVM_GUEST_RAM_MMAP_ZAP_COMPLETE \
	_IOW(0, 18, eq_microvm_guest_ram_mmap_zap_op_t)
#define EQ_INSTANCE_MICROVM_STOP _IOW(0, 19, eq_microvm_stop_arg_t)
#define EQ_INSTANCE_MICROVM_BOOT _IOW(0, 20, eq_microvm_boot_arg_t)
#define EQ_SHMGET _IOW(1, 2, eq_shmget_arg_t)
#define EQ_SHMCTL _IOW(1, 3, eq_shmctl_arg_t)
