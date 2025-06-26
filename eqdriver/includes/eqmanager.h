#pragma once

#ifndef __KERNEL__
# include <sys/ioctl.h>
# include <stdint.h>
#endif

#define EQMANAGER_NAME "eqmanager"

#define MAX_EQ_INSTANCES_NUM (64)

typedef struct eq_create_instance_arg
{
    uint64_t instance_id; // Instance ID, set by kernel driver.
    uint64_t instance_type; // Instance type
    uint64_t mapping_type; // Mapping type, e.g., course-grained mapping or one-to-one mapping
} eq_create_instance_arg_t;

typedef struct eq_remove_instance_arg
{
    uint64_t instance_id; // Instance ID
} eq_remove_instance_arg_t;

#define EQ_CREATE_INSTANCE _IOW(0, 0, eq_create_instance_arg_t)
#define EQ_REMOVE_INSTANCE _IOW(0, 1, eq_remove_instance_arg_t)

int init_eqmanagement_device(void);
void exit_eqmanagement_device(void);

int create_instance(eq_create_instance_arg_t *arg);