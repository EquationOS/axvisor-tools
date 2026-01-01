
#pragma once

#include "eqmanager.h"

#define STATUS_CREATED (0)
#define STATUS_SETTING_UP (1)
#define STATUS_RUNNING (2)
#define STATUS_STOPPED (3)

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
    /// Base GPA of the microVM's memory region.
    uint64_t memory_region_base_gpa;
    /// Size of the microVM's memory region.
    uint64_t memory_region_size;

    /// Initial number of vCPUs for the microVM.
    uint64_t init_vcpu_num;
    /// Maximum number of vCPUs for the microVM.
    uint64_t max_vcpu_num;
} eq_instance_metadata_t;

int create_instance(eq_create_instance_arg_t *arg);
int remove_instance(int instance_id);

void instances_init(void);
void instances_exit(void);