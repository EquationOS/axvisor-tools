
#pragma once

#include "eqmanager.h"

#define STATUS_CREATED (0)
#define STATUS_SETTING_UP (1)
#define STATUS_RUNNING (2)
#define STATUS_STOPPED (3)
#define EQ_MAX_PASSTHROUGH_DEVICES (8)
#define EQ_MAX_VFIO_BARS (6)
#define EQ_MAX_PCI_CFG_SPACE_BYTES (256)

/// See `EqInstanceMetadata` in `equation_defs/src/configs.rs`
typedef struct eq_instance_metadata
{    
    /* For LibOS instance */
    
    /// Base GPA of the SCF region.
    /// This region is used for syscall-forward between the LibOS and the host.
    uint64_t scf_region_base_gpa;
    /// Size of the SCF region.
    uint64_t scf_region_size;
    /// Base GPA of the page cache pool region,
    /// used for page cache of the LibOS when it operates file I/O on host files.
    uint64_t page_cache_pool_base_gpa;
    /// Size of the page cache pool region.
    uint64_t page_cache_pool_size;

    /* For microVM instance */
    /// Size of the microVM's memory region (set by eqdriver).
    uint64_t init_memory_region_size_mib;
    /// Maximum size of the microVM's memory region (set by eqdriver).
    uint64_t max_memory_region_size_mib;
    /// Base GPA of the microVM's memory region.
    uint64_t memory_region_base_gpa;

    /// Initial number of vCPUs for the microVM.
    uint64_t init_vcpu_num;
    /// Maximum number of vCPUs for the microVM.
    uint64_t max_vcpu_num;
    /// Number of encoded passthrough BDF entries.
    uint64_t passthrough_device_count;
    /// Encoded BDF list, each entry is 0xddddbbddf.
    uint64_t passthrough_bdf[EQ_MAX_PASSTHROUGH_DEVICES];
    /// VFIO metadata for passthrough backend selection.
    uint64_t vfio_flags;
    uint64_t vfio_iommu_group;
    uint64_t vfio_guest_visible_bdf;
    uint64_t vfio_bar_count;
    uint64_t vfio_bar_start[EQ_MAX_VFIO_BARS];
    uint64_t vfio_bar_size[EQ_MAX_VFIO_BARS];
    uint64_t vfio_bar_flags[EQ_MAX_VFIO_BARS];
    /// Snapshot of host PCI config space bytes used for guest probe emulation.
    uint64_t vfio_pci_cfg_space_len;
    uint8_t vfio_pci_cfg_space[EQ_MAX_PCI_CFG_SPACE_BYTES];
} eq_instance_metadata_t;

int create_instance(eq_create_instance_arg_t *arg);
int remove_instance(int instance_id);

void instances_init(void);
void instances_exit(void);