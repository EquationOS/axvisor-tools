#include <asm/cacheflush.h>
#include <asm/tlbflush.h>
#include <linux/fs.h>
#include <linux/io.h>
#include <linux/list.h>
#include <linux/memory.h>
#include <linux/miscdevice.h>
#include <linux/mutex.h>
#include <linux/slab.h>
#include <linux/uaccess.h>
#include <stdatomic.h>

#include "includes/hvc.h"
#include "includes/ivc.h"
#include "includes/ring.h"
#include "includes/utils.h"


struct axivc_publisher_vdev
{
	struct miscdevice misc;
	char name[64];
	int id;
	bool active;
	uint64_t key;
	uint64_t shm_base;
	uint64_t shm_size;
	void __iomem *mapped_shm_base;

	struct list_head list;
};

struct axivc_subscriber_vdev
{
	struct miscdevice misc;
	char name[64];
	int id;
	bool active;
	uint64_t publisher_id;
	uint64_t key;
	uint64_t shm_base;
	uint64_t shm_size;
	void __iomem *mapped_shm_base;

	struct list_head list;
};

static int pub_vdev_count = 0;
static DEFINE_MUTEX(pub_vdev_lock);

static LIST_HEAD(pub_vdev_list_head);
static int next_pub_vdev_id = 0;

static int sub_vdev_count = 0;
static DEFINE_MUTEX(sub_vdev_lock);
static LIST_HEAD(sub_vdev_list_head);
static int next_sub_vdev_id = 0;

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

int ivc_unregister_publisher_vdev(struct axivc_publisher_vdev *vdev);
int ivc_unregister_subscriber_vdev(struct axivc_subscriber_vdev *vdev);

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
axivc_manager_ioctl(struct file *file, unsigned int cmd, unsigned long arg);

/**
 * @brief File operations structure for the axvisor IVC publisher device.
 */
static const struct file_operations axivc_publisher_fops;

/**
 * @brief File operations structure for the axvisor IVC subscriber device.
 */
static const struct file_operations axivc_subscriber_fops;

int ivc_publish_channel(
	uint64_t channel_key, uint64_t expected_shm_size, char *pub_dev_name)
{
	int ret;
	int id;
	uint64_t shm_base = 0;
	uint64_t shm_size = expected_shm_size;
	struct axivc_publisher_vdev *vdev;
	void __iomem *mapped_shm_base;

	INFO(
		"axvisor: Initializing IVC channel with key: 0x%llx, expected size "
		"0x%llx\n",
		channel_key, expected_shm_size);

	// Call the hypervisor to publish the channel
	ret = hvc_publish_channel(
		(u64)channel_key, kva2pa((u64)&shm_base), kva2pa((u64)&shm_size));
	if (ret != 0)
	{
		ERROR("axvisor: Failed to publish channel, error code: %d\n", ret);
		return -EIO;
	}

	INFO(
		"axvisor: IVC publish channel allocated successfully, base: 0x%llx, "
		"size: 0x%llx key 0x%llx\n",
		shm_base, shm_size, channel_key);

	// Map the shared memory base to kernel space.
	// This assumes that the shared memory is already allocated and accessible.
	// The shared memory base should be a physical address.
	mapped_shm_base = ioremap(shm_base, shm_size);
	if (!mapped_shm_base)
	{
		ERROR("axvisor: Failed to map shared memory base\n");
		return -ENOMEM;
	}

	mutex_lock(&pub_vdev_lock);

	if (pub_vdev_count >= MAX_VDEVS)
	{
		iounmap(mapped_shm_base);
		mutex_unlock(&pub_vdev_lock);
		return -ENOMEM;
	}

	// id = pub_vdev_count;
	id = next_pub_vdev_id++;

	vdev = kzalloc(sizeof(*vdev), GFP_KERNEL);
	if (!vdev)
	{
		iounmap(mapped_shm_base);
		mutex_unlock(&pub_vdev_lock);
		return -ENOMEM;
	}
	snprintf(
		vdev->name, sizeof(vdev->name), "%s%d", IVC_PUBLISHER_DEV_NAME_PREFIX,
		id);
	snprintf(
		pub_dev_name, sizeof(vdev->name), "/dev/%s%d",
		IVC_PUBLISHER_DEV_NAME_PREFIX, id);

	vdev->misc.minor = MISC_DYNAMIC_MINOR;
	vdev->misc.name = vdev->name;
	vdev->misc.fops = &axivc_publisher_fops;
	vdev->id = id;
	vdev->key = channel_key;
	vdev->shm_base = shm_base;
	vdev->shm_size = shm_size;
	vdev->mapped_shm_base = mapped_shm_base;
	vdev->active = false;

	// Init ring buffer in shared memory region from publisher side.
	shm_ring_init(mapped_shm_base, shm_size, channel_key);

	ret = misc_register(&vdev->misc);
	if (ret)
	{
		kfree(vdev);
		iounmap(mapped_shm_base);
		mutex_unlock(&pub_vdev_lock);
		return ret;
	}

	INIT_LIST_HEAD(&vdev->list);
	list_add_tail(&vdev->list, &pub_vdev_list_head);

	pub_vdev_count++;

	mutex_unlock(&pub_vdev_lock);

	INFO(
		"manage: created vdev %s (id=%d) {ket = %llx}\n", vdev->name, vdev->id,
		vdev->key);

	return 0;
}

int ivc_unpublish_channel(uint64_t channel_key)
{
	int ret;
	bool found = false;
	struct axivc_publisher_vdev *vdev, *tmp;

	INFO("axvisor: Unregistering IVC channel with key: 0x%llx\n", channel_key);

	// Call the hypervisor to unregister the channel
	ret = hvc_unpublish_channel(channel_key);
	if (ret != 0)
	{
		ERROR("axvisor: Failed to unregister channel, error code: %d\n", ret);
		return -EIO;
	}

	// Unregister the publisher device
	list_for_each_entry_safe(vdev, tmp, &pub_vdev_list_head, list)
	{
		if (vdev->key == channel_key)
		{
			found = true;
			// Unmap the shared memory base from kernel space.
			iounmap(vdev->mapped_shm_base);
			// Unregister the publisher device
			ret = ivc_unregister_publisher_vdev(vdev);
			if (ret < 0)
			{
				ERROR(
					"axvisor: Failed to unregister publisher device %s, "
					"error code: %d\n",
					vdev->name, ret);
				return ret;
			}

			break;
		}
	}
	if (!found)
	{
		ERROR(
			"axvisor: No publisher device found with key: 0x%llx\n",
			channel_key);
		return -ENOENT;
	}

	INFO("axvisor: IVC channel unregistered successfully\n");
	return 0;
}

int ivc_unregister_publisher_vdev(struct axivc_publisher_vdev *vdev)
{
	if (!vdev)
		return -EINVAL;

	INFO(
		"axvisor: Unregistering publisher device %s (id=%d) key %llx\n",
		vdev->name, vdev->id, vdev->key);

	if (vdev->active)
	{
		ERROR(
			"axvisor: Cannot unregister active publisher device %s (id=%d), "
			"please close it first\n",
			vdev->name, vdev->id);
		return -EBUSY;
	}

	mutex_lock(&pub_vdev_lock);

	misc_deregister(&vdev->misc);
	list_del(&vdev->list);
	kfree(vdev);

	pub_vdev_count--;
	mutex_unlock(&pub_vdev_lock);

	return 0;
}

int ivc_subscribe_channel(u64 publisher_id, u64 key, char *sub_dev_name)
{
	int ret;
	int id;
	uint64_t shm_base = 0;
	uint64_t shm_size = 0;
	struct axivc_subscriber_vdev *vdev;
	void __iomem *mapped_shm_base;

	INFO("axvisor: Subscribing to IVC channel with key: 0x%llx\n", key);

	// Call the hypervisor to subscribe to the channel
	ret = hvc_subscribe_channel(
		publisher_id, key, kva2pa((u64)&shm_base), kva2pa((u64)&shm_size));
	if (ret != 0)
	{
		ERROR("axvisor: Failed to subscribe to channel, error code: %d\n", ret);
		return -EIO;
	}

	INFO(
		"axvisor: IVC subscribtion channel init successfully, base: 0x%llx, "
		"size: 0x%llx\n",
		shm_base, shm_size);

	// Map the shared memory base to kernel space.
	// This assumes that the shared memory is already allocated and accessible.
	// The shared memory base should be a physical address.
	mapped_shm_base = ioremap(shm_base, shm_size);
	if (!mapped_shm_base)
	{
		ERROR("axvisor: Failed to map shared memory base\n");
		return -ENOMEM;
	}
	mutex_lock(&sub_vdev_lock);

	if (sub_vdev_count >= MAX_VDEVS)
	{
		iounmap(mapped_shm_base);
		mutex_unlock(&sub_vdev_lock);
		return -ENOMEM;
	}

	id = next_sub_vdev_id++;
	vdev = kzalloc(sizeof(*vdev), GFP_KERNEL);
	if (!vdev)
	{
		iounmap(mapped_shm_base);
		mutex_unlock(&sub_vdev_lock);
		return -ENOMEM;
	}

	snprintf(
		vdev->name, sizeof(vdev->name), "%s%d", IVC_SUBSCRIBER_DEV_NAME_PREFIX,
		id);
	snprintf(
		sub_dev_name, sizeof(vdev->name), "/dev/%s%d",
		IVC_SUBSCRIBER_DEV_NAME_PREFIX, id);

	vdev->misc.minor = MISC_DYNAMIC_MINOR;
	vdev->misc.name = vdev->name;
	vdev->misc.fops = &axivc_subscriber_fops;
	vdev->id = id;
	vdev->publisher_id = publisher_id;
	vdev->key = key;
	vdev->shm_base = shm_base;
	vdev->shm_size = shm_size;
	vdev->mapped_shm_base = mapped_shm_base;
	vdev->active = false;

	ret = misc_register(&vdev->misc);
	if (ret)
	{
		kfree(vdev);
		iounmap(mapped_shm_base);
		mutex_unlock(&sub_vdev_lock);
		return ret;
	}

	INIT_LIST_HEAD(&vdev->list);
	list_add_tail(&vdev->list, &sub_vdev_list_head);
	sub_vdev_count++;
	mutex_unlock(&sub_vdev_lock);

	INFO(
		"manage: created subscriber vdev %s (id=%d) {publisher_id = %llu, key "
		"= "
		"0x%llx}\n",
		vdev->name, vdev->id, vdev->publisher_id, vdev->key);

	return 0;
}

int ivc_unsubscribe_channel(u64 publisher_id, u64 key)
{
	int ret;
	bool found = false;
	struct axivc_subscriber_vdev *vdev, *tmp;

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

	// Unregister the subscriber device
	list_for_each_entry_safe(vdev, tmp, &sub_vdev_list_head, list)
	{
		if (vdev->publisher_id == publisher_id && vdev->key == key)
		{
			found = true;
			// Unmap the shared memory base from kernel space.
			iounmap(vdev->mapped_shm_base);
			// Unregister the subscriber device
			ret = ivc_unregister_subscriber_vdev(vdev);
			if (ret < 0)
			{
				ERROR(
					"axvisor: Failed to unregister subscriber device %s, "
					"error code: %d\n",
					vdev->name, ret);
				return ret;
			}
			break;
		}
	}
	if (!found)
	{
		ERROR(
			"axvisor: No subscriber device found with publisher_id: %llu and "
			"key: 0x%llx\n",
			publisher_id, key);
		return -ENOENT;
	}

	INFO("axvisor: IVC channel %llu unsubscribed successfully\n", publisher_id);
	return 0;
}

int ivc_unregister_subscriber_vdev(struct axivc_subscriber_vdev *vdev)
{
	if (!vdev)
		return -EINVAL;

	INFO(
		"axvisor: Unregistering subscriber device %s publisher_id %lld key "
		"%llx\n",
		vdev->name, vdev->publisher_id, vdev->key);

	if (vdev->active)
	{
		ERROR(
			"axvisor: Cannot unregister active subscriber device %s (id=%d), "
			"please close it first\n",
			vdev->name, vdev->id);
		return -EBUSY;
	}

	mutex_lock(&sub_vdev_lock);

	misc_deregister(&vdev->misc);
	list_del(&vdev->list);
	kfree(vdev);

	sub_vdev_count--;
	mutex_unlock(&sub_vdev_lock);

	return 0;
}

static ssize_t axivc_publisher_read(
	struct file *file, char __user *buf, size_t count, loff_t *ppos)
{
	struct axivc_publisher_vdev *vdev = file->private_data;
	char msg[128];
	size_t len;

	if (!vdev->active)
	{
		ERROR(
			"axvisor: Device %s is not active, cannot write to shared memory\n",
			vdev->name);
		return -ENODEV;
	}

	snprintf(msg, sizeof(msg), "Hello from AXIVC publisher %s!\n", vdev->name);
	len = strlen(msg);

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
	struct axivc_publisher_vdev *vdev = file->private_data;
	void __iomem *mapped_shm_base = vdev->mapped_shm_base;
	shm_ring_t *ring = (shm_ring_t *)mapped_shm_base;
	size_t ret;

	// Validate the count to ensure it does not exceed the shared memory size.
	// if (count > publisher_shm_size)
	// {
	// 	ERROR("axvisor: Write size exceeds shared memory size\n");
	// 	return -EINVAL;
	// }

	if (!vdev->active)
	{
		ERROR(
			"axvisor: Device %s is not active, cannot write to shared memory\n",
			vdev->name);
		return -ENODEV;
	}

	INFO(
		"axvisor: Try to write %zu bytes to IVCChannel key [%llx]\n", count,
		vdev->key);

	// Check shared memory header.
	if (ring->key != vdev->key)
	{
		ERROR(
			"axvisor: Shared memory header key mismatch, expected: 0x%llx, "
			"got: 0x%llx\n",
			vdev->key, ring->key);
		return -EINVAL;
	}

	ret = shm_ring_enqueue(mapped_shm_base, buf, count);
	if (ret < 0)

	{
		if (ret == -EAGAIN)
		{
			ERROR(
				"axvisor: Shared memory ring buffer is full, cannot enqueue "
				"data\n");
			return ret; // Return error if the ring buffer is full
		}
		else
		{
			ERROR(
				"axvisor: Failed to enqueue data to shared ring buffer, error "
				"code: %ld\n",
				ret);
			return ret; // Return other error codes
		}
	}

	// Flush the cache to ensure data is written to shared memory.
	flush_cache_vmap(
		(unsigned long)mapped_shm_base,
		(unsigned long)mapped_shm_base + vdev->shm_size);

	INFO(
		"axvisor: Written %zu bytes to IVCChannel key [%llx]\n", ret,
		vdev->key);

	return ret;
}

static int axivc_publisher_open(struct inode *inode, struct file *file)
{
	struct axivc_publisher_vdev *vdev;

	vdev = container_of(file->private_data, struct axivc_publisher_vdev, misc);
	file->private_data = vdev;

	vdev->active = true;

	INFO(
		"axvisor: Opened device %s, publisher_shm_base: 0x%llx, "
		"publisher_shm_size: 0x%llx\n",
		vdev->name, vdev->shm_base, vdev->shm_size);

	return 0;
}

static int axivc_publisher_release(struct inode *inode, struct file *file)
{
	struct axivc_publisher_vdev *vdev = file->private_data;
	INFO("axvisor: Closing device %s\n", vdev->name);
	if (vdev->active)
	{
		vdev->active = false;
		INFO("axvisor: Device %s is now inactive\n", vdev->name);
	}
	else
	{
		WARNING(
			"axvisor: Device %s was already inactive, check why?\n",
			vdev->name);
	}
	return 0;
}

static int axivc_subscriber_open(struct inode *inode, struct file *file)
{
	struct axivc_subscriber_vdev *vdev;
	vdev = container_of(file->private_data, struct axivc_subscriber_vdev, misc);
	file->private_data = vdev;
	vdev->active = true;

	// Check if the device is opened with write permission
	if ((file->f_flags & O_ACCMODE) == O_WRONLY ||
		(file->f_flags & O_ACCMODE) == O_RDWR)
	{
		ERROR(
			"Subscriber %s cannot be opened with write permission\n",
			vdev->name);
		return -EPERM; // Return permission error
	}

	INFO("axvisor: Opened device %s success\n", vdev->name);
	return 0;
}

static int axivc_subscriber_release(struct inode *inode, struct file *file)
{
	struct axivc_subscriber_vdev *vdev = file->private_data;
	if (vdev->active)
	{
		vdev->active = false;
		INFO("axvisor: Device %s is now inactive\n", vdev->name);
	}
	else
	{
		WARNING(
			"axvisor: Device %s was already inactive, check why?\n",
			vdev->name);
	}

	return 0;
}

static ssize_t axivc_subscriber_read(
	struct file *file, char __user *buf, size_t count, loff_t *ppos)
{
	struct axivc_subscriber_vdev *vdev = file->private_data;
	void __iomem *mapped_shm_base = vdev->mapped_shm_base;
	shm_ring_t *ring = (shm_ring_t *)mapped_shm_base;
	size_t bytes_read = 0;
	int ret;

	if (!vdev->active)
	{
		ERROR(
			"axvisor: Device %s is not active, cannot read from shared "
			"memory\n",
			vdev->name);
		return -ENODEV;
	}
	if (ring->key != vdev->key)
	{
		ERROR(
			"axvisor: Shared memory header key mismatch, expected: 0x%llx, "
			"got: 0x%llx\n",
			vdev->key, ring->key);
		return -EINVAL;
	}
	INFO(
		"axvisor: Try to read %ld bytes from IVCChannel ID [%lld] key [%llx]\n",
		count, vdev->publisher_id, vdev->key);

	ret = shm_ring_dequeue(mapped_shm_base, buf, count, &bytes_read);
	if (ret < 0)
	{
		ERROR(
			"axvisor: Failed to read data from shared ring buffer, error "
			"code: %d\n",
			ret);
		return ret; // Return error code
	}

	return bytes_read; // Return number of bytes read
}

static long
axivc_manager_ioctl(struct file *file, unsigned int ioctl, unsigned long arg)
{
	int ret = 0;
	ivc_publish_arg_t publish_arg;
	ivc_subscribe_arg_t subscribe_arg;
	uint64_t shm_size = 0;

	switch (ioctl)
	{
	case IVC_PUBLISH_CHANNEL:
		if (copy_from_user(
				&publish_arg, (u64 __user *)arg, sizeof(ivc_publish_arg_t)))
		{
			ERROR("axvisor: Failed to copy channel key from user space\n");
			return -EFAULT;
		}

		INFO(
			"axvisor: Publishing channel with key: 0x%llx, size: 0x%llx\n",
			publish_arg.channel_key, publish_arg.channel_size);

		shm_size = publish_arg.channel_size;
		// Initialize publisher shared memory channel.
		if (ivc_publish_channel(
				publish_arg.channel_key, shm_size, publish_arg.device_name))
		{
			ERROR(
				"axvisor: Failed to publish channel with key: 0x%llx\n",
				publish_arg.channel_key);
			return -EIO;
		}

		if (copy_to_user(
				(u64 __user *)arg, &publish_arg, sizeof(ivc_publish_arg_t)))
		{
			ERROR("axvisor: Failed to copy channel key to user space\n");
			return -EFAULT;
		}

		break;
	case IVC_UNPUBLISH_CHANNEL:
		if (copy_from_user(
				&publish_arg, (u64 __user *)arg, sizeof(ivc_publish_arg_t)))
		{
			ERROR("axvisor: Failed to copy channel key from user space\n");
			return -EFAULT;
		}

		INFO(
			"axvisor: Unpublishing channel with key: 0x%llx\n",
			publish_arg.channel_key);

		ret = ivc_unpublish_channel(publish_arg.channel_key);
		if (ret)
		{
			ERROR(
				"axvisor: Failed to unpublish channel with key: 0x%llx\n",
				publish_arg.channel_key);
			return ret;
		}

		break;
	case IVC_SUBSCRIBE_CHANNEL:
		if (copy_from_user(
				&subscribe_arg, (u64 __user *)arg, sizeof(ivc_subscribe_arg_t)))
		{
			ERROR("axvisor: Failed to copy channel key from user space\n");
			return -EFAULT;
		}

		INFO(
			"axvisor: Subscribing to channel [%llu] with key: 0x%llx\n",
			subscribe_arg.target_publisher_id, subscribe_arg.channel_key);

		ret = ivc_subscribe_channel(
			subscribe_arg.target_publisher_id, subscribe_arg.channel_key,
			subscribe_arg.device_name);
		if (ret)
		{
			ERROR(
				"axvisor: Failed to subscribe to channel %llu\n",
				subscribe_arg.target_publisher_id);
			return ret;
		}

		if (copy_to_user(
				(u64 __user *)arg, &subscribe_arg, sizeof(ivc_subscribe_arg_t)))
		{
			ERROR("axvisor: Failed to copy channel key to user space\n");
			return -EFAULT;
		}
		break;
	case IVC_UNSUBSCRIBE_CHANNEL:
		if (copy_from_user(
				&subscribe_arg, (u64 __user *)arg, sizeof(ivc_subscribe_arg_t)))
		{
			ERROR("axvisor: Failed to copy channel key from user space\n");
			return -EFAULT;
		}

		INFO(
			"axvisor: Unsubscribing from channel [%llu] with key: 0x%llx\n",
			subscribe_arg.target_publisher_id, subscribe_arg.channel_key);

		ret = ivc_unsubscribe_channel(
			subscribe_arg.target_publisher_id, subscribe_arg.channel_key);
		if (ret)
		{
			ERROR(
				"axvisor: Failed to unsubscribe from channel %llu\n",
				subscribe_arg.target_publisher_id);
			return ret;
		}

		break;
	default:
		ERROR("axvisor: Invalid ioctl command\n");
		return -EINVAL;
	}

	return ret;
}

static int axivc_manager_open(struct inode *inode, struct file *file)
{
	INFO("axvisor: Opened device %s success\n", IVC_DEV_NAME);
	return 0;
}

static int axivc_manager_release(struct inode *inode, struct file *file)
{
	int ret = 0;
	INFO("axvisor: Closing device %s\n", IVC_DEV_NAME);
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
	// .unlocked_ioctl = axivc_subscriber_ioctl,
	// .compat_ioctl = axivc_subscriber_ioctl,
	.release = axivc_subscriber_release,
};

static const struct file_operations axivc_manager_fops = {
	.owner = THIS_MODULE,
	.open = axivc_manager_open,
	.unlocked_ioctl = axivc_manager_ioctl,
	.compat_ioctl = axivc_manager_ioctl,
	.release = axivc_manager_release,
};

static struct miscdevice axvisor_ivc_management_vdev = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = IVC_DEV_NAME,
	.fops = &axivc_manager_fops,
};

int init_ivc_devices()
{
	int ret = 0;

	// Register a IVC management device.
	ret = misc_register(&axvisor_ivc_management_vdev);
	if (ret)
	{
		WARNING(
			"axvisor: Failed to register misc device %s\n",
			axvisor_ivc_management_vdev.name);
		return ret;
	}
	INFO(
		"axvisor: IVC management device registered with name %s\n",
		axvisor_ivc_management_vdev.name);

	return ret;
}

void uninit_ivc_devices()
{
	struct axivc_publisher_vdev *pvdev, *ptmp;
	struct axivc_subscriber_vdev *svdev, *stmp;
	int ret;

	INFO("%s\n", __func__);

	// Cleanup the publisher devices.
	list_for_each_entry_safe(pvdev, ptmp, &pub_vdev_list_head, list)
	{
		// Call the hypervisor to unregister the channel
		ret = hvc_unpublish_channel(pvdev->key);
		if (ret != 0)
		{
			ERROR(
				"axvisor: Failed to unregister publish channel key 0x%llx, "
				"error code: %d\n",
				pvdev->key, ret);
		}
		ivc_unregister_publisher_vdev(pvdev);
	}
	// Cleanup the subscriber devices.
	list_for_each_entry_safe(svdev, stmp, &sub_vdev_list_head, list)
	{
		// Call the hypervisor to unregister the channel
		ret = hvc_unsubscribe_channel(svdev->publisher_id, svdev->key);
		if (ret != 0)
		{
			ERROR(
				"axvisor: Failed to unsubscribe from channel %llu with key "
				"0x%llx, error code: %d\n",
				svdev->publisher_id, svdev->key, ret);
		}
		ivc_unregister_subscriber_vdev(svdev);
	}

	misc_deregister(&axvisor_ivc_management_vdev);

	INFO("axvisor driver unloaded\n");
}