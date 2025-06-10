#include <linux/module.h>
/**
 * @file main.c
 * @brief A simple Linux kernel module that registers a misc device named "axvisor_vdev".
 *
 * This module demonstrates the creation of a misc device with basic read functionality.
 * When the device is read, it returns a static message to the user space.
 *
 * @author axvisor
 * @license GPL
 */

#include <linux/miscdevice.h>
#include <linux/fs.h>
#include <linux/uaccess.h>

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
static ssize_t axvisor_read(struct file *file, char __user *buf, size_t count, loff_t *ppos);

/**
 * @brief File operations structure for the misc device.
 *
 * This structure defines the operations supported by the misc device.
 * Currently, only the read operation is implemented.
 */
static const struct file_operations axvisor_fops;

/**
 * @brief Misc device structure for "axvisor_vdev".
 *
 * This structure defines the misc device, including its name and file operations.
 */
static struct miscdevice axvisor_vdev;

/**
 * @brief Module initialization function.
 *
 * This function is called when the module is loaded into the kernel.
 * It registers the misc device and logs the initialization status.
 *
 * @return 0 on success, or a negative error code on failure.
 */
static int __init axvisor_init(void);

/**
 * @brief Module cleanup function.
 *
 * This function is called when the module is removed from the kernel.
 * It deregisters the misc device and logs the cleanup status.
 */
static void __exit axvisor_exit(void);

MODULE_AUTHOR("axvisor");
MODULE_LICENSE("GPL");

#define DEVICE_NAME "axvisor_vdev"

static ssize_t axvisor_read(struct file *file, char __user *buf, size_t count, loff_t *ppos) {
    const char *msg = "Hello from axvisor_vdev!\n";
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

static const struct file_operations axvisor_fops = {
    .owner = THIS_MODULE,
    .read = axvisor_read,
};

static struct miscdevice axvisor_vdev = {
    .minor = MISC_DYNAMIC_MINOR,
    .name = DEVICE_NAME,
    .fops = &axvisor_fops,
};

static int __init axvisor_init(void) {
    int ret;

    pr_info("axvisor: Initializing axvisor_vdev\n");

    ret = misc_register(&axvisor_vdev);
    if (ret) {
        pr_err("axvisor: Failed to register misc device\n");
        return ret;
    }

    pr_info("axvisor: Device registered with name %s\n", DEVICE_NAME);
    return 0;
}

static void __exit axvisor_exit(void) {
    pr_info("axvisor: Exiting axvisor_vdev\n");
    misc_deregister(&axvisor_vdev);
}

module_init(axvisor_init);
module_exit(axvisor_exit);
