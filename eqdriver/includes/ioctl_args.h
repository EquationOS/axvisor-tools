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
#define EQ_MAX_PASSTHROUGH_DEVICES (8)
#define EQ_MAX_VFIO_BARS (6)
#define EQ_MAX_PCI_CFG_SPACE_BYTES (256)

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
	uint64_t vfio_flags; // bit0: enabled, bit1: iova-gpa-identity
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
} eq_create_instance_arg_t;

typedef struct eq_remove_instance_arg
{
	uint64_t instance_id; // Instance ID
} eq_remove_instance_arg_t;

typedef struct eq_instance_irq_inject_arg
{
	uint64_t instance_id; // Target instance ID, 0 means current instance fd owner
	uint32_t msix_index;  // MSI-X entry index raised on host VFIO side
	uint32_t reserved;    // Reserved for alignment/future flags
} eq_instance_irq_inject_arg_t;

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
#define EQ_SHMGET _IOW(1, 2, eq_shmget_arg_t)
#define EQ_SHMCTL _IOW(1, 3, eq_shmctl_arg_t)