#pragma once

#ifndef __KERNEL__
#include <stdint.h>
#include <sys/ioctl.h>
#endif

#define EQMANAGER_NAME "eqmanager"

#define EQINSTANCE_DEV_PREFIX "eqinstance_"

#define MAX_EQ_INSTANCES_NUM (64)

typedef struct eq_create_instance_arg
{
	uint64_t instance_id;	// Instance ID, set by kernel driver.
	uint64_t instance_type; // Instance type
	uint64_t mapping_type;	// Mapping type, e.g., course-grained mapping or
							// one-to-one mapping
} eq_create_instance_arg_t;

typedef struct eq_remove_instance_arg
{
	uint64_t instance_id; // Instance ID
} eq_remove_instance_arg_t;

#define EQ_CREATE_INSTANCE _IOW(0, 0, eq_create_instance_arg_t)
#define EQ_REMOVE_INSTANCE _IOW(0, 1, eq_remove_instance_arg_t)