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
#include "includes/shm.h"
#include "includes/utils.h"

typedef struct eq_instance_vdev
{
	struct miscdevice misc;
	char name[64];
	// Instance ID, unique identifier for the instance
	int id;
	bool active;

	struct list_head scf_region_head;

	struct list_head shm_list_head; // Head of the shared memory list
	// current pos of eqshm_t in the list
	eqshm_t *current_shm;
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

static int instance_scf_buf_mmap(struct file *file, struct vm_area_struct *vma)
{
	eq_instance_vdev_t *instance_vdev = file->private_data;
	int ret = 0;
	pid_t pid = task_pid_nr(current);
	const char *comm = current->comm;
	eqscf_queue_region_t *scf_region;
	unsigned long pfn_start, size;

	scf_region =
		get_scf_queue_region_by_host_pid(&instance_vdev->scf_region_head, pid);

	if (!scf_region)
	{
		ERROR(
			"SCF queue region for PID %d (%s) not found in instance %s\n", pid,
			comm, instance_vdev->name);
		return -ENODEV;
	}
	pfn_start = scf_region->base_gpa >> PAGE_SHIFT;
	size = scf_region->size;

	INFO(
		"[%s] SCF queue region for PID %d (%s) found in instance %s, "
		"base_gpa: 0x%llx, size: 0x%lx\n",
		__func__, pid, comm, instance_vdev->name, scf_region->base_gpa, size);

	ret =
		remap_pfn_range(vma, vma->vm_start, pfn_start, size, vma->vm_page_prot);

	if (ret)
		ERROR(
			"%s: remap_pfn_range failed at [0x%lx  0x%lx]\n", __func__,
			vma->vm_start, vma->vm_end);

	return ret;
}

static int instance_mmap(struct file *file, struct vm_area_struct *vma)
{
	eq_instance_vdev_t *instance_vdev = file->private_data;
	int ret = 0;
	unsigned long pfn_start;
	unsigned long size;
	unsigned long prot = pgprot_val(vma->vm_page_prot);

	u64 base_addr = 0;
	unsigned long offset = vma->vm_pgoff << PAGE_SHIFT;
	int requested_page_count = 0;

	int instance_id = instance_vdev->id;

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

	INFO(
		"[%s] called by PID %d (%s) for instance %s, [0x%lx, 0x%lx), "
		"offset: 0x%lx vm_pgoff 0x%lx\n",
		__func__, task_pid_nr(current), current->comm, instance_vdev->name,
		vma->vm_start, vma->vm_end, offset, vma->vm_pgoff);

	// Check if the offset is the SCF magic number.
	// If so, we will map the SCF queue region.
	// This is a special case for SCF queue regions.
	if (vma->vm_pgoff == SCF_MAGIC_NUMBER)
	{
		return instance_scf_buf_mmap(file, vma);
	}

	// Count pages number.
	requested_page_count = (vma->vm_end - vma->vm_start) >> PAGE_SHIFT;

	base_addr = (__u64)allocate_contiguous_shm_pages(
		instance_vdev->current_shm, requested_page_count);

	if (base_addr == 0)
	{
		WARNING(
			"Failed to allocate %d pages for instance %d, [0x%lx, 0x%lx)\n",
			requested_page_count, instance_id, vma->vm_start, vma->vm_end);
	}

	size = vma->vm_end - vma->vm_start;

	INFO("vm_flags: 0x%lx, vm_page_prot: 0x%lx\n", vma->vm_flags, prot);

	// First, hvc to sync the mapping with the Instance guest addrspace in
	// AxVisor.
	ret = hvc_load_mmap(
		instance_id, vma->vm_start, base_addr, size, (__u64)vma->vm_flags,
		prot);
	if (ret < 0)
	{
		ERROR(
			"%s: hvc_sync_mmap failed for instance %d, error code: %d\n",
			__func__, instance_id, ret);
		return ret;
	}

	pfn_start = (base_addr >> PAGE_SHIFT) + vma->vm_pgoff;
	// Set RW permissions for the mapping.
	// This is necessary for the ELF loader in axcli to write to the
	// memory region.
	// The correct permission will be set by `mprotect` in axcli after the
	// loading is done.
	prot |= _PAGE_RW;
	vma->vm_page_prot = __pgprot(prot);

	INFO(
		"[remap_pfn_range] virt:0x%lx phy: 0x%lx, offset: 0x%lx, size: 0x%lx "
		"(%d pages)\n",
		vma->vm_start, pfn_start << PAGE_SHIFT, offset, size,
		requested_page_count);

	ret =
		remap_pfn_range(vma, vma->vm_start, pfn_start, size, vma->vm_page_prot);

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
	eqshm_t *shm;
	phys_addr_t shm_base_ptr_gpa, scf_queue_base_ptr_gpa;
	eqscf_queue_region_t *init_scf;
	__u64 *shm_base = kmalloc(sizeof(__u64), GFP_KERNEL);
	__u64 *scf_queue_base = kmalloc(sizeof(__u64), GFP_KERNEL);

	pid_t pid = task_pid_nr(current);
	const char *comm = current->comm;

	INFO("[mmap] called by PID %d (%s)\n", pid, comm);

	shm_base_ptr_gpa = virt_to_phys(shm_base);
	scf_queue_base_ptr_gpa = virt_to_phys(scf_queue_base);

	if (!shm_base)
		return -ENOMEM;

	INFO(
		"shm_base gva @ %p, gpa @ %llx\n", shm_base,
		(unsigned long long)shm_base_ptr_gpa);

	// Create a new instance through the hypervisor call.
	instance_id = hvc_create_instance(
		arg->instance_type, arg->mapping_type, shm_base_ptr_gpa,
		scf_queue_base_ptr_gpa);

	INFO(
		"Creating instance with type %llu, mapping type %llu, shm base @ "
		"0x%llx scf queue base @ 0x%llx\n",
		arg->instance_type, arg->mapping_type, *shm_base, *scf_queue_base);

	if (instance_id < 0)
	{
		ERROR(
			"Failed to create instance through hypervisor, error code: %d\n",
			instance_id);
		ret = instance_id;
		goto err_free_shm_base;
	}

	if (instance_id >= MAX_EQ_INSTANCES_NUM)
	{
		ERROR(
			"Instance ID %d exceeds maximum allowed instances %d\n",
			instance_id, MAX_EQ_INSTANCES_NUM);
		ret = -EINVAL;
		goto err_free_shm_base;
	}
	// Set the instance ID in the argument structure,
	// which will be copied back to user space.
	// This is necessary for the user space to know the assigned instance ID.
	arg->instance_id = (uint64_t)instance_id;

	instance_vdev = &instances_array[arg->instance_id];

	if (instance_vdev->active)
	{
		// The instance is already active, but AxVisor still assigned this
		// instance ID, which means that the instance has been removed
		// but not yet cleaned up.
		INFO(
			"Instance %d is already active, but it was removed, reusing it\n",
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
		goto err_free_shm_base;
	}

	instance_vdev->id = instance_id;
	instance_vdev->active = true;

	// Initialize the SCF queue region list head.
	INIT_LIST_HEAD(&instance_vdev->scf_region_head);
	init_scf = kzalloc(sizeof(eqscf_queue_region_t), GFP_KERNEL);
	if (!init_scf)
	{
		ERROR(
			"Failed to allocate eqscf_queue_region_t for instance %d\n",
			instance_id);
		ret = -ENOMEM;
		goto err_free_shm_base;
	}
	init_scf->base_gpa = *scf_queue_base;
	init_scf->pid = 1;
	init_scf->host_pid = pid;
	init_scf->size = 0x200000; // 2 MB
	INIT_LIST_HEAD(&init_scf->list);
	list_add_tail(&init_scf->list, &instance_vdev->scf_region_head);

	// Initialize the shared memory list head.
	INIT_LIST_HEAD(&instance_vdev->shm_list_head);

	// Allocate the first shared memory region.
	shm = allocate_new_shm_region(
		&instance_vdev->shm_list_head, (void *)*shm_base);
	if (!shm)
	{
		ERROR(
			"Failed to allocate shared memory region for instance %d\n",
			instance_id);
		ret = -ENOMEM;
		goto err_free_shm_base;
	}
	instance_vdev->current_shm = shm;

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
		goto err_free_shm_base;
	}

	INFO(
		"Created instance %s with ID %d\n", instance_vdev->name,
		instance_vdev->id);

err_free_shm_base:
	kfree(shm_base);

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

	// Traverse and release shared memory regions
	eqshm_t *shm, *tmp;
	list_for_each_entry_safe(shm, tmp, &vdev->shm_list_head, list)
	{
		cleanup_shm_region(shm, &vdev->shm_list_head);
	}

	// Traverse and release SCF queue regions
	eqscf_queue_region_t *scf_region, *tmp_scf;
	list_for_each_entry_safe(scf_region, tmp_scf, &vdev->scf_region_head, list)
	{
		list_del(&scf_region->list);
		kfree(scf_region);
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