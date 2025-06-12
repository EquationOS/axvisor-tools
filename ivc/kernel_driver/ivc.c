#include <asm/cacheflush.h>
#include <asm/tlbflush.h>
#include <linux/fs.h>
#include <linux/io.h>
#include <linux/memory.h>
#include <linux/miscdevice.h>
#include <linux/uaccess.h>

#include "includes/hvc.h"
#include "includes/ivc.h"
#include "includes/utils.h"

/// Example channel key
u64 publisher_channel_key = 0xdeadbeef;
/// Shared memory base physical address.
u64 publisher_shm_base = 0;
/// Shared memory size in bytes.
u64 publisher_shm_size = 0;

u64 subscriber_channel_id = 0;
u64 subscriber_channel_key = 0;
/// Shared memory base physical address for subscriber.
u64 subscriber_shm_base = 0;
/// Shared memory size in bytes for subscriber.
u64 subscriber_shm_size = 0;

/**
 * @brief Read operation for the axvisor IVC publisher device.
 *
 * Read data from IVC publisher device is not valid,
 * as a publisher, you can only write data to the device.
 */
static ssize_t axivc_publisher_read(
	struct file *file, char __user *buf, size_t count, loff_t *ppos);

/**
 * @brief Write operation for the axvisor IVC publisher device.
 *
 * This function is called when the user writes to the IVC publisher device.
 * It copies data from the user buffer to the IVC shared memory region.
 *
 * @param file Pointer to the file structure.
 * @param buf User-space buffer containing data to write.
 * @param count Number of bytes to write.
 * @param ppos Pointer to the file position.
 * @return Number of bytes written on success, or a negative error code on
 * failure.
 */
static ssize_t axivc_publisher_write(
	struct file *file, const char __user *buf, size_t count, loff_t *ppos);

/**
 * @brief Open operation for the axvisor IVC publisher device.
 *
 * The IVC channel is already established during module initialization,
 * so this function will just check if the shared memory base and size are
 * initialized. If they are not, it will return an error.
 */
static int axivc_publisher_open(struct inode *inode, struct file *file);

/**
 * @brief Release operation for the axvisor IVC publisher device.
 *
 * It will NOT close the IVC channel by calling `ivc_unpublish_channel()`,
 * as the channel is expected to remain open until the module is unloaded.
 * Instead, it will just log the closure of the device.
 */
static int axivc_publisher_release(struct inode *inode, struct file *file);

/**
 * @brief Read operation for the axvisor IVC subscriber device.
 *
 * This function is called when the user reads from the IVC subscriber device.
 * It reads data from the IVC shared memory region and copies it to the user
 * buffer.
 */
static ssize_t axivc_subscriber_read(
	struct file *file, char __user *buf, size_t count, loff_t *ppos);

/**
 * @brief Open operation for the axvisor IVC subscriber device.
 *
 * This function is expected to be called by the user client when it wants to
 * subscribe to a IVC channel, the channel will not be established immediately,
 * the user client is expected to configure the target publisher ID and
 * channel key through `ioctl`.
 */
static int axivc_subscriber_open(struct inode *inode, struct file *file);

/**
 * @brief Release operation for the axvisor IVC subscriber device.
 *
 * It will close the IVC channel by calling `ivc_unsubscribe_channel()`.
 * This function is expected to be called by the user client when it no longer
 * needs to subscribe to the IVC channel.
 * It will also log the closure of the device.
 */
static int axivc_subscriber_release(struct inode *inode, struct file *file);

/**
 * @brief IOCTL operation for the axvisor IVC subscriber device.
 *
 * This function handles the IOCTL commands for the IVC subscriber device,
 * allowing the user client to subscribe or unsubscribe from a channel, and
 * set the target publisher ID.
 *
 * @param file Pointer to the file structure.
 * @param cmd The command to execute.
 * @param arg The argument for the command.
 * @return 0 on success, or a negative error code on failure.
 */
static long
axivc_subscriber_ioctl(struct file *file, unsigned int cmd, unsigned long arg);

/**
 * @brief File operations structure for the axvisor IVC publisher device.
 */
static const struct file_operations axivc_publisher_fops;

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

	INFO(
		"axvisor: Initializing IVC channel with key: 0x%llx\n",
		publisher_channel_key);

	// Call the hypervisor to publish the channel
	ret = hvc_publish_channel(
		publisher_channel_key, kva2pa((u64)&publisher_shm_base),
		kva2pa((u64)&publisher_shm_size));
	if (ret != 0)
	{
		ERROR("axvisor: Failed to publish channel, error code: %d\n", ret);
		return -EIO;
	}

	INFO(
		"axvisor: IVC publish channel initialized successfully, base: 0x%llx, "
		"size: 0x%llx\n",
		publisher_shm_base, publisher_shm_size);

	return 0;
}

int ivc_unpublish_channel(void)
{
	int ret;

	INFO(
		"axvisor: Unregistering IVC channel with key: 0x%llx\n",
		publisher_channel_key);

	// Call the hypervisor to unregister the channel
	ret = hvc_unpublish_channel(publisher_channel_key);
	if (ret != 0)
	{
		ERROR("axvisor: Failed to unregister channel, error code: %d\n", ret);
		return -EIO;
	}

	publisher_shm_base = 0;
	publisher_shm_size = 0;

	INFO("axvisor: IVC channel unregistered successfully\n");
	return 0;
}

int ivc_subscribe_channel(u64 publisher_id, u64 key)
{
	int ret;

	INFO("axvisor: Subscribing to IVC channel with key: 0x%llx\n", key);

	// Call the hypervisor to subscribe to the channel
	ret = hvc_subscribe_channel(
		publisher_id, key, kva2pa((u64)&subscriber_shm_base),
		kva2pa((u64)&subscriber_shm_size));
	if (ret != 0)
	{
		ERROR("axvisor: Failed to subscribe to channel, error code: %d\n", ret);
		return -EIO;
	}

	INFO(
		"axvisor: IVC subscribtion channel init successfully, base: 0x%llx, "
		"size: 0x%llx\n",
		subscriber_shm_base, subscriber_shm_size);

	return 0;
}

int ivc_unsubscribe_channel(u64 publisher_id, u64 key)
{
	int ret;

	INFO(
		"axvisor: Unsubscribing from IVC channel %llu with key: 0x%llx\n",
		publisher_id, key);

	// Call the hypervisor to unsubscribe from the channel
	ret = hvc_unsubscribe_channel(publisher_id, key);
	if (ret != 0)
	{
		ERROR(
			"axvisor: Failed to unsubscribe from channel %llu, error code: "
			"%d\n",
			publisher_id, ret);
		return -EIO;
	}

	INFO("axvisor: IVC channel %llu unsubscribed successfully\n", publisher_id);
	return 0;
}

static ssize_t axivc_publisher_read(
	struct file *file, char __user *buf, size_t count, loff_t *ppos)
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

static ssize_t axivc_publisher_write(
	struct file *file, const char __user *buf, size_t count, loff_t *ppos)
{
	void __iomem *mapped_shm_base;
	void __iomem *mapped_shm_buf;
	struct ivc_shm_header *mapped_shm_header;

	// Ensure the shared memory base and size are initialized.
	if (publisher_shm_base == 0 || publisher_shm_size == 0)
	{
		ERROR("axvisor: Shared memory base or size is not initialized\n");
		return -EINVAL;
	}

	// Validate the count to ensure it does not exceed the shared memory size.
	if (count > publisher_shm_size)
	{
		ERROR("axvisor: Write size exceeds shared memory size\n");
		return -EINVAL;
	}

	// Map the shared memory base to kernel space.
	// This assumes that the shared memory is already allocated and accessible.
	// The shared memory base should be a physical address.
	mapped_shm_base = ioremap(publisher_shm_base, publisher_shm_size);
	if (!mapped_shm_base)
	{
		ERROR("axvisor: Failed to map shared memory base\n");
		return -ENOMEM;
	}
	mapped_shm_header = (struct ivc_shm_header *)mapped_shm_base;
	mapped_shm_buf = mapped_shm_base + sizeof(struct ivc_shm_header);

	INFO(
		"axvisor: Try to write %zu bytes to shared memory, key %llx\n", count,
		mapped_shm_header->key);

	// Check shared memory header.
	if (mapped_shm_header->key != publisher_channel_key)
	{
		ERROR("axvisor: Shared memory header publisher ID mismatch\n");
		iounmap(mapped_shm_base);
		return -EINVAL;
	}

	if (copy_from_user(mapped_shm_buf, buf, count))
	{
		ERROR("axvisor: Failed to copy data from user space\n");
		iounmap(mapped_shm_base);
		return -EFAULT;
	}
	// Update the shared memory header with content size.
	mapped_shm_header->content_size = count;

	// Flush the cache to ensure data is written to shared memory.
	flush_cache_vmap(
		(unsigned long)mapped_shm_base,
		(unsigned long)mapped_shm_base + sizeof(struct ivc_shm_header) + count);

	INFO("axvisor: Written %zu bytes to shared memory\n", count);
	iounmap(mapped_shm_base);
	return count;
}

static int axivc_publisher_open(struct inode *inode, struct file *file)
{
	if (publisher_shm_base == 0 || publisher_shm_size == 0)
	{
		ERROR("axvisor: Shared memory base or size is not initialized\n");
		return -EINVAL;
	}

	INFO(
		"axvisor: Opened device %s, publisher_shm_base: 0x%llx, "
		"publisher_shm_size: 0x%llx\n",
		IVC_PUBLISHER_DEV_NAME, publisher_shm_base, publisher_shm_size);

	return 0;
}

static int axivc_publisher_release(struct inode *inode, struct file *file)
{
	INFO("axvisor: Closing device %s\n", IVC_PUBLISHER_DEV_NAME);
	return 0;
}

static int axivc_subscriber_open(struct inode *inode, struct file *file)
{
	// Check if the device is opened with write permission
	if ((file->f_flags & O_ACCMODE) == O_WRONLY ||
		(file->f_flags & O_ACCMODE) == O_RDWR)
	{
		ERROR(
			"Subscriber %s cannot be opened with write permission\n",
			IVC_SUBSCRIBER_DEV_NAME);
		return -EPERM; // Return permission error
	}

	INFO("axvisor: Opened device %s success\n", IVC_SUBSCRIBER_DEV_NAME);
	return 0;
}

static int axivc_subscriber_release(struct inode *inode, struct file *file)
{
	int ret;

	INFO("axvisor: Closing device %s\n", IVC_SUBSCRIBER_DEV_NAME);

	// Check if need to unsubscribe from the IVC channel.
	if (subscriber_channel_id != 0 && subscriber_channel_key != 0)
	{
		INFO(
			"axvisor: Unsubscribing from channel [%llu] with key: 0x%llx\n",
			subscriber_channel_id, subscriber_channel_key);

		ret = hvc_unsubscribe_channel(
			subscriber_channel_id, subscriber_channel_key);
		if (ret != 0)
		{
			ERROR(
				"axvisor: Failed to unsubscribe from channel, error code: %d\n",
				ret);
			return -EIO;
		}
	}

	return 0;
}

static ssize_t axivc_subscriber_read(
	struct file *file, char __user *buf, size_t count, loff_t *ppos)
{
	void __iomem *mapped_shm_base;
	void __iomem *mapped_shm_buf;
	struct ivc_shm_header *mapped_shm_header;
	size_t bytes_read;

	// Ensure the shared memory base and size are initialized.
	if (subscriber_shm_base == 0 || subscriber_shm_size == 0)
	{
		ERROR(
			"axvisor: Subscriber Shared memory base or size is not "
			"initialized\n");
		return -EINVAL;
	}

	// Map the shared memory base to kernel space.
	mapped_shm_base = ioremap(subscriber_shm_base, subscriber_shm_size);
	if (!mapped_shm_base)
	{
		ERROR("axvisor: Failed to map shared memory base\n");
		return -ENOMEM;
	}
	mapped_shm_header = (struct ivc_shm_header *)mapped_shm_base;
	mapped_shm_buf = mapped_shm_base + sizeof(struct ivc_shm_header);

	if (mapped_shm_header->content_size == 0)
	{
		iounmap(mapped_shm_base);
		return 0; // No data to read
	}

	if (count < mapped_shm_header->content_size)
	{
		iounmap(mapped_shm_base);
		return -EINVAL; // Not enough space in user buffer
	}
	bytes_read = mapped_shm_header->content_size > count
					 ? count
					 : mapped_shm_header->content_size;

	if (copy_to_user(buf, mapped_shm_buf, bytes_read))
	{
		iounmap(mapped_shm_base);
		return -EFAULT; // Failed to copy data to user space
	}

	iounmap(mapped_shm_base);
	return bytes_read; // Return number of bytes read
}

static long
axivc_subscriber_ioctl(struct file *file, unsigned int ioctl, unsigned long arg)
{
	int ret = 0;

	struct ivc_subscribe_arg subscribe_arg;

	switch (ioctl)
	{
	case IVC_SUBSCRIBE_CHANNEL:
		if (copy_from_user(
				&subscribe_arg, (u64 __user *)arg,
				sizeof(struct ivc_subscribe_arg)))
		{
			ERROR("axvisor: Failed to copy channel key from user space\n");
			return -EFAULT;
		}

		INFO(
			"axvisor: Subscribing to channel [%llu] with key: 0x%llx\n",
			subscribe_arg.target_publisher_id, subscribe_arg.channel_key);

		ret = ivc_subscribe_channel(
			subscribe_arg.target_publisher_id, subscribe_arg.channel_key);
		if (ret)
		{
			ERROR(
				"axvisor: Failed to subscribe to channel %llu\n",
				subscribe_arg.target_publisher_id);
			return ret;
		}

		if (subscriber_shm_base == 0 || subscriber_shm_size == 0)
		{
			ERROR(
				"axvisor: Subscriber shared memory base or size is not "
				"initialized\n");
			return -EINVAL;
		}

		// Store the subscriber channel ID and key for future use.
		subscriber_channel_id = subscribe_arg.target_publisher_id;
		subscriber_channel_key = subscribe_arg.channel_key;

		INFO(
			"axvisor: Subscribing to channel [%llu] with key: 0x%llx "
			"success!!\n",
			subscriber_channel_id, subscriber_channel_key);

		break;

	case IVC_UNSUBSCRIBE_CHANNEL:
		if (copy_from_user(
				&subscribe_arg, (u64 __user *)arg,
				sizeof(struct ivc_subscribe_arg)))
		{
			ERROR("axvisor: Failed to copy channel key from user space\n");
			return -EFAULT;
		}

		INFO(
			"axvisor: Unsubscribing from channel [%llu] with key: 0x%llx\n",
			subscriber_channel_id, subscriber_channel_key);
		ret = ivc_unsubscribe_channel(
			subscriber_channel_id, subscriber_channel_key);
		if (ret)
		{
			ERROR(
				"axvisor: Failed to unsubscribe from channel%llu\n",
				subscriber_channel_id);
			return ret;
		}
		// Reset the subscriber channel ID and key.
		subscriber_channel_id = 0;
		subscriber_channel_key = 0;
		break;
	default:
		ERROR("axvisor: Invalid ioctl command\n");
		return -EINVAL;
	}

	return ret;
}

static const struct file_operations axivc_publisher_fops = {
	.owner = THIS_MODULE,
	.open = axivc_publisher_open,
	.read = axivc_publisher_read,
	.write = axivc_publisher_write,
	.release = axivc_publisher_release,
};

static const struct file_operations axivc_subscriber_fops = {
	.owner = THIS_MODULE,
	.open = axivc_subscriber_open,
	.read = axivc_subscriber_read,
	.unlocked_ioctl = axivc_subscriber_ioctl,
	.compat_ioctl = axivc_subscriber_ioctl,
	.release = axivc_subscriber_release,
};

static struct miscdevice axvisor_ivc_publisher_vdev = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = IVC_PUBLISHER_DEV_NAME,
	.fops = &axivc_publisher_fops,
};

static struct miscdevice axvisor_ivc_subscriber_vdev = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = IVC_SUBSCRIBER_DEV_NAME,
	.fops = &axivc_subscriber_fops,
};

int init_ivc_devices()
{
	int ret = 0;

	// Register a publisher device.
	ret = misc_register(&axvisor_ivc_publisher_vdev);
	if (ret)
	{
		WARNING(
			"axvisor: Failed to register misc device %s\n",
			IVC_PUBLISHER_DEV_NAME);
		return ret;
	}
	INFO(
		"axvisor: IVC publisher device registered with name %s\n",
		IVC_PUBLISHER_DEV_NAME);
	// Initialize publisher shared memory channel.
	ivc_publish_channel();

	// Register a subscriber device.
	ret = misc_register(&axvisor_ivc_subscriber_vdev);
	if (ret)
	{
		WARNING(
			"axvisor: Failed to register misc device %s\n",
			IVC_SUBSCRIBER_DEV_NAME);
		return ret;
	}
	INFO(
		"axvisor: IVC subscriber device registered with name %s\n",
		IVC_SUBSCRIBER_DEV_NAME);

	// DO NOT initialize subscriber shared memory channel here,
	// as it is expected to be initialized by user client whenever it is needed.

	return ret;
}

void uninit_ivc_devices()
{
	misc_deregister(&axvisor_ivc_publisher_vdev);
	// Unregister the channel during cleanup.
	ivc_unpublish_channel();

	misc_deregister(&axvisor_ivc_subscriber_vdev);
}