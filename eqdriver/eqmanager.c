#include <linux/fs.h>
#include <linux/io.h>
#include <linux/miscdevice.h>
#include <linux/uaccess.h>

#include "includes/eqmanager.h"
#include "includes/instance.h"
#include "includes/shm.h"
#include "includes/utils.h"

static int eqmanager_open(struct inode *inode, struct file *file)
{
	INFO("axvisor: Opened device %s success\n", EQMANAGER_NAME);
	return 0;
}

static int eqmanager_release(struct inode *inode, struct file *file)
{
	int ret = 0;
	INFO("axvisor: Closing device %s\n", EQMANAGER_NAME);
	return ret;
}

static long
eqmanager_ioctl(struct file *file, unsigned int ioctl, unsigned long arg)
{
	int ret = 0;
	switch (ioctl)
	{
	case EQ_CREATE_INSTANCE:
		eq_create_instance_arg_t create_arg;
		if (copy_from_user(
				&create_arg, (__u64 __user *)arg,
				sizeof(eq_create_instance_arg_t)))
		{
			ERROR("Failed to create arg from user space\n");
			return -EFAULT;
		}

		ret = create_instance(&create_arg);
		if (ret < 0)
		{
			ERROR("Failed to create instance: %d\n", ret);
			return ret;
		}

		if (copy_to_user(
				(__u64 __user *)arg, &create_arg,
				sizeof(eq_create_instance_arg_t)))
		{
			ERROR("Failed to instance ID to user space\n");
			return -EFAULT;
		}

		break;

	case EQ_REMOVE_INSTANCE:
		eq_remove_instance_arg_t remove_arg;
		if (copy_from_user(
				&remove_arg, (__u64 __user *)arg,
				sizeof(eq_remove_instance_arg_t)))
		{
			ERROR("Failed to remove arg from user space\n");
			return -EFAULT;
		}

		ret = remove_instance((int)remove_arg.instance_id);
		if (ret < 0)
		{
			ERROR("Failed to remove instance: %d\n", ret);
			return ret;
		}

		break;

	case EQ_SHMGET:
	{
		eq_shmget_arg_t shmget_arg;
		if (copy_from_user(
				&shmget_arg, (__u64 __user *)arg, sizeof(eq_shmget_arg_t)))
		{
			ERROR("Failed to get shmget arg from user space\n");
			return -EFAULT;
		}

		ret = eq_shmget(
			(key_t)shmget_arg.key, (size_t)shmget_arg.size,
			(int)shmget_arg.shmflg);
		if (ret < 0)
		{
			ERROR("Failed to get shared memory: %d\n", ret);
			return ret;
		}
		break;
	}
	case EQ_SHMCTL:
	{
		eq_shmctl_arg_t shmctl_arg;
		if (copy_from_user(
				&shmctl_arg, (__u64 __user *)arg, sizeof(eq_shmctl_arg_t)))
		{
			ERROR("Failed to get shmctl arg from user space\n");
			return -EFAULT;
		}
		ERROR("shmctl: shmid=%llu, cmd=%llu, buf=%llu, not implemented\n",
			shmctl_arg.shmid, shmctl_arg.cmd, shmctl_arg.buf);
		return -ENOSYS; // Not implemented yet
		break;
	}

	default:
		ERROR("Invalid ioctl command\n");
		return -EINVAL;
	}

	return ret;
}

/// Used for mapping AxVisor globel shared memory region to user space processed
/// such as IOKerneld.
static int eqmanager_mmap(struct file *file, struct vm_area_struct *vma)
{
	// This function is a placeholder for memory mapping functionality.
	// Currently, it does not implement any specific memory mapping logic.
	INFO("Memory mapping requested for device %s\n", EQMANAGER_NAME);
	key_t key = vma->vm_pgoff; // Use vm_pgoff as the key for shared memory
	return eq_shmat(file, vma, key);
}

static const struct file_operations eq_manager_fops = {
	.owner = THIS_MODULE,
	.open = eqmanager_open,
	.unlocked_ioctl = eqmanager_ioctl,
	.compat_ioctl = eqmanager_ioctl,
	.release = eqmanager_release,
	.mmap = eqmanager_mmap,
};

static struct miscdevice eq_management_dev = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = EQMANAGER_NAME,
	.fops = &eq_manager_fops,
};

int init_eqmanagement_device(void)
{
	int ret = 0;

	INFO("%s\n", __func__);

	// Hypercall to told the AxVisor to initialize the shared memory region.

	// Register the eqmanager device.
	ret = misc_register(&eq_management_dev);
	if (ret)
	{
		WARNING("Failed to register misc device %s\n", eq_management_dev.name);
		return ret;
	}
	INFO("\"/dev/%s\" registered successful\n", eq_management_dev.name);

	return ret;
}

void exit_eqmanagement_device(void)
{
	// Unregister the eqmanager device.
	misc_deregister(&eq_management_dev);

	INFO("%s unregistered successful\n", eq_management_dev.name);
}