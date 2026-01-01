#include <linux/fs.h>
#include <linux/io.h>
#include <linux/list.h>
#include <linux/miscdevice.h>
#include <linux/mm.h>
#include <linux/pgtable.h>
#include <linux/pid.h>	 // for pid_nr()
#include <linux/sched.h> // for current

#include "includes/eqmanager.h"
#include "includes/hvc.h"
#include "includes/instance.h"
#include "includes/utils.h"

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

	eq_instance_metadata_t metadata;
} eq_instance_vdev_t;

static eq_instance_vdev_t instances_array[MAX_EQ_INSTANCES_NUM];

int unregister_instance_dev(eq_instance_vdev_t *vdev);

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

/// @brief Map the shared memory region for the instance.
/// This function will be called when `axcli` calls `mmap` on the instance fd
/// ("/dev/eqinstance_<instance_id>").
static int instance_mmap(struct file *file, struct vm_area_struct *vma)
{
	eq_instance_vdev_t *instance_vdev = file->private_data;
	int ret = 0;
	int instance_id = instance_vdev->id;
	unsigned long pfn_start, mmap_size;

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
		// For microVM instances, we only have one memory region to map.
		if (mmap_size > instance_vdev->metadata.memory_region_size)
		{
			ERROR(
				"MicroVM memory region size 0x%llx is smaller than requested "
				"mmap "
				"size "
				"0x%lx\n",
				instance_vdev->metadata.memory_region_size, mmap_size);
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
			instance_vdev->metadata.memory_region_base_gpa +
				instance_vdev->metadata.memory_region_size,
			instance_vdev->metadata.memory_region_size);
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
	.mmap = instance_mmap,
	.release = instance_dev_release,
};

int create_instance(eq_create_instance_arg_t *arg)
{
	int instance_id;
	int ret = 0;
	eq_instance_vdev_t *instance_vdev;

	eq_instance_metadata_t *instance_metadata;
	phys_addr_t instance_metadata_ptr_gpa;

	// pid_t pid = task_pid_nr(current);
	// const char *comm = current->comm;

	instance_metadata = kmalloc(sizeof(eq_instance_metadata_t), GFP_KERNEL);
	if (!instance_metadata)
	{
		ERROR("Failed to allocate memory for instance metadata\n");
		ret = -ENOMEM;
		return ret;
	}

	instance_metadata_ptr_gpa = virt_to_phys(instance_metadata);

	memset(instance_metadata, 0, sizeof(eq_instance_metadata_t));

	instance_metadata->memory_region_size = arg->memory_size;

	// Create a new instance through the hypervisor call.
	instance_id = hvc_create_instance(
		arg->instance_type, arg->mapping_type, instance_metadata_ptr_gpa);

	if (arg->instance_type < 2)
	{

		INFO(
			"Creating LibOS instance %d with type %llu, mapping type %llu\n"
			"scf queue base @ 0x%llx, size 0x%llx\n"
			"page cache base @ 0x%llx, size 0x%llx\n",
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
			"memory region base @ 0x%llx, size 0x%llx, vcpu number %lld\n",
			instance_id, instance_metadata->memory_region_base_gpa,
			instance_metadata->memory_region_size,
			instance_metadata->init_vcpu_num);
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

	memcpy(
		&instance_vdev->metadata, instance_metadata,
		sizeof(eq_instance_metadata_t));

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