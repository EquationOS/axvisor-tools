
#pragma once

#include "eqmanager.h"

#define STATUS_CREATED (0)
#define STATUS_SETTING_UP (1)
#define STATUS_RUNNING (2)
#define STATUS_STOPPED (3)

/// See `EqInstanceMetadata` in `equation_defs/src/configs.rs`
typedef struct eq_instance_metadata
{    
    uint64_t scf_region_base_gpa;
    uint64_t scf_region_size;

    uint64_t page_cache_pool_base_gpa;
    uint64_t page_cache_pool_size;
} eq_instance_metadata_t;

int create_instance(eq_create_instance_arg_t *arg);
int remove_instance(int instance_id);

void instances_init(void);
void instances_exit(void);