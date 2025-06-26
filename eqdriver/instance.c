#include <linux/miscdevice.h>

#include "includes/eqmanager.h"
#include "includes/hvc.h"

typedef struct eq_instance_vdev
{
	struct miscdevice misc;
	char name[64];
	int id;
	bool active;
	uint64_t shm_base;
	uint64_t shm_size;
	void __iomem *mapped_shm_base;
} eq_instance_vdev_t;

static eq_instance_vdev_t instances_array[MAX_EQ_INSTANCES_NUM];

int create_instance(eq_create_instance_arg_t *arg)
{
	int instance_id;

	// Create a new instance through the hypervisor call.
	instance_id = hvc_create_instance(arg->instance_type, arg->mapping_type);

	if (instance_id < 0)
	{
		ERROR(
			"Failed to create instance through hypervisor, error code: %d\n",
			instance_id);
		return instance_id;
	}

	if (instance_id >= MAX_EQ_INSTANCES_NUM)
	{
		ERROR(
			"Instance ID %d exceeds maximum allowed instances %d\n",
			instance_id, MAX_EQ_INSTANCES_NUM);
		return -EINVAL;
	}
	arg->instance_id = (uint64_t)instance_id;

	eq_instance_vdev_t *instance_vdev = &instances_array[arg->instance_id];
	instance_vdev->id = instance_id;
	instance_vdev->active = false;
	instance_vdev->shm_base = 0; // Set a default shared memory base
	instance_vdev->shm_size = 0; // Set a default shared memory size
	instance_vdev->mapped_shm_base = NULL;

	snprintf(
		instance_vdev->name, sizeof(instance_vdev->name), "eq_instance_%d",
		instance_vdev->id);

	INFO(
		"Created instance %s with ID %d\n", instance_vdev->name,
		instance_vdev->id);
	return 0;
}

void instances_init(void)
{
	// Just clear the instances array to ensure no garbage data.
	memset(instances_array, 0, sizeof(instances_array));
}