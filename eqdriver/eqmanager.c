#include <linux/fs.h>
#include <linux/io.h>
#include <linux/miscdevice.h>
#include <linux/uaccess.h>

#include "includes/eqmanager.h"
#include "includes/instance.h"
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
	eq_create_instance_arg_t create_arg;

	switch (ioctl)
	{
	case EQ_CREATE_INSTANCE:
		if (copy_from_user(
				&create_arg, (__u64 __user *)arg,
				sizeof(eq_create_instance_arg_t)))
		{
			ERROR("Failed to create arg from user space\n");
			return -EFAULT;
		}

		INFO("EQ_CREATE_INSTANCE id %lld type %lld\n", create_arg.instance_id, create_arg.instance_type);

		ret = create_instance(&create_arg);
		if (ret < 0)
		{
			ERROR("Failed to create instance: %d\n", ret);
			return ret;
		}

		create_arg.instance_id = 123; // Simulate instance ID assignment

		if (copy_to_user(
				(__u64 __user *)arg, &create_arg,
				sizeof(eq_create_instance_arg_t)))
		{
			ERROR("Failed to instance ID to user space\n");
			return -EFAULT;
		}

		break;
	default:
		ERROR("Invalid ioctl command\n");
		return -EINVAL;
	}

	return ret;
}

static const struct file_operations eq_manager_fops = {
	.owner = THIS_MODULE,
	.open = eqmanager_open,
	.unlocked_ioctl = eqmanager_ioctl,
	.compat_ioctl = eqmanager_ioctl,
	.release = eqmanager_release,
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