#include <linux/fs.h>
#include <linux/io.h>
#include <linux/memory.h>
#include <linux/miscdevice.h>
#include <linux/uaccess.h>

#include "hvc.h"
#include "ivc.h"
#include "utils.h"

/// Example channel key
u64 channel_key = 0xdeadbeef;
/// Shared memory base physical address.
u64 shm_base = 0;
/// Shared memory size in bytes.
u64 shm_size = 0;

/**
 * @brief Read operation for the misc device.
 *
 * This function is called when the user reads from the device file.
 * It copies a static message to the user buffer.
 *
 * @param file Pointer to the file structure.
 * @param buf User-space buffer to copy data to.
 * @param count Number of bytes to read.
 * @param ppos Pointer to the file position.
 * @return Number of bytes read on success, or a negative error code on failure.
 */
static ssize_t
axvisor_read(struct file *file, char __user *buf, size_t count, loff_t *ppos);

/**
 * @brief Write operation for the misc device.
 *
 * This function is called when the user writes to the device file.
 * It copies data from the user buffer to a shared memory region.
 *
 * @param file Pointer to the file structure.
 * @param buf User-space buffer containing data to write.
 * @param count Number of bytes to write.
 * @param ppos Pointer to the file position.
 * @return Number of bytes written on success, or a negative error code on
 * failure.
 */
static ssize_t axvisor_write(
	struct file *file, const char __user *buf, size_t count, loff_t *ppos);

/**
 * @brief Open operation for the misc device.
 *
 * This function is called when the device file is opened.
 * It initializes shared memory base and size.
 *
 * @param inode Pointer to the inode structure.
 * @param file Pointer to the file structure.
 * @return 0 on success, or a negative error code on failure.
 */
static int axvisor_open(struct inode *inode, struct file *file);

/**
 * @brief Close operation for the misc device.
 *
 * This function is called when the device file is closed.
 * It unregisters the IVC channel.
 *
 * @param inode Pointer to the inode structure.
 * @param file Pointer to the file structure.
 * @return 0 on success, or a negative error code on failure.
 */
static int axvisor_close(struct inode *inode, struct file *file);

/**
 * @brief File operations structure for the misc device.
 *
 * This structure defines the operations supported by the misc device.
 * Currently, only the read operation is implemented.
 */
static const struct file_operations axvisor_fops;

/**
 * @brief Misc device structure for "axvisor_ivc_publisher_vdev".
 *
 * This structure defines the misc device, including its name and file
 * operations.
 */
static struct miscdevice axvisor_ivc_publisher_vdev;

int ivc_publish_channel(void)
{
	int ret;

	pr_err("axvisor: Initializing IVC channel with key: 0x%llx\n", channel_key);

	// Call the hypervisor to publish the channel
	ret = hvc_publish_channel(
		channel_key, kva2pa((u64)&shm_base), kva2pa((u64)&shm_size));
	if (ret != 0)
	{
		pr_err("axvisor: Failed to publish channel, error code: %d\n", ret);
		return -EIO;
	}

	pr_err(
		"axvisor: IVC channel initialized successfully, base: 0x%llx, size: "
		"0x%llx\n",
		shm_base, shm_size);

	return 0;
}

int ivc_unregister_channel(void)
{
	int ret;

	pr_err(
		"axvisor: Unregistering IVC channel with key: 0x%llx\n", channel_key);

	// Call the hypervisor to unregister the channel
	ret = hvc_call(HIVCUnPublishChannel, channel_key, 0, 0, 0, 0, 0);
	if (ret != 0)
	{
		pr_err("axvisor: Failed to unregister channel, error code: %d\n", ret);
		return -EIO;
	}

	pr_err("axvisor: IVC channel unregistered successfully\n");
	return 0;
}

static ssize_t
axvisor_read(struct file *file, char __user *buf, size_t count, loff_t *ppos)
{
	const char *msg = "Hello from axvisor_ivc_publisher_vdev!\n";
	size_t len = strlen(msg);

	if (*ppos >= len)
		return 0;

	if (count > len - *ppos)
		count = len - *ppos;

	if (copy_to_user(buf, msg + *ppos, count))
		return -EFAULT;

	*ppos += count;
	return count;
}

static ssize_t axvisor_write(
	struct file *file, const char __user *buf, size_t count, loff_t *ppos)
{
	void __iomem *mapped_shm_base;

	if (count > shm_size)
	{
		pr_err("axvisor: Write size exceeds shared memory size\n");
		return -EINVAL;
	}

	mapped_shm_base = ioremap(shm_base, shm_size);
	if (!mapped_shm_base)
	{
		pr_err("axvisor: Failed to map shared memory base\n");
		return -ENOMEM;
	}

	if (copy_from_user(mapped_shm_base, buf, count))
	{
		pr_err("axvisor: Failed to copy data from user space\n");
		iounmap(mapped_shm_base);
		return -EFAULT;
	}

	pr_info("axvisor: Written %zu bytes to shared memory\n", count);
	iounmap(mapped_shm_base);
	return count;
}

static int axvisor_open(struct inode *inode, struct file *file)
{
	// Initialize shared memory base and size
	ivc_publish_channel();
	if (shm_base == 0 || shm_size == 0)
	{
		pr_err("axvisor: Shared memory base or size is not initialized\n");
		return -EINVAL;
	}

	pr_info(
		"axvisor: Opened device %s, shm_base: 0x%llx, shm_size: 0x%llx\n",
		IVC_PUBLISHER_DEV_NAME, shm_base, shm_size);

	return 0;
}

static int axvisor_close(struct inode *inode, struct file *file)
{
	// Unregister the channel when closing the device
	ivc_unregister_channel();
	pr_info("axvisor: Closed device %s\n", IVC_PUBLISHER_DEV_NAME);
	return 0;
}

static const struct file_operations axvisor_fops = {
	.owner = THIS_MODULE,
	.open = axvisor_open,
	.read = axvisor_read,
	.write = axvisor_write,
	.release = axvisor_close,
};

static struct miscdevice axvisor_ivc_publisher_vdev = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = IVC_PUBLISHER_DEV_NAME,
	.fops = &axvisor_fops,
};

int init_ivc_devices()
{
	int ret = 0;
	ret = misc_register(&axvisor_ivc_publisher_vdev);
	if (ret)
	{
		pr_err(
			"axvisor: Failed to register misc device %s\n",
			IVC_PUBLISHER_DEV_NAME);
		return ret;
	}

	pr_info(
		"axvisor: Device registered with name %s\n", IVC_PUBLISHER_DEV_NAME);

	return ret;
}

void uninit_ivc_devices() { misc_deregister(&axvisor_ivc_publisher_vdev); }