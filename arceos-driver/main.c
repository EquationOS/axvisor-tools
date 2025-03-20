#include <linux/module.h>
#include <linux/interrupt.h>
#include <linux/kernel.h>

MODULE_AUTHOR("arceos");
MODULE_LICENSE("GPL");

static int __init arceos_init(void) {
    int err;
    
    pr_info(LOGHEAD "Initializing...\n");

    pr_info(LOGHEAD "Registering arceos_vdev...\n");
    err = misc_register(&arceos_vdev);
    if (err) {
        pr_err(LOGHEAD "Cannot register arceos_vdev! err: %d\n", err);
        goto end;
    }

    pr_info(LOGHEAD "Registering irq...\n");
    err = request_irq(ARCEOS_VIRQ, arceos_virq_handler, IRQF_SHARED, "arceos_hypervisor", &arceos_vdev);
    if (err) {
        pr_err(LOGHEAD "Cannot register irq! err: %d\n", err);
        goto err_unregister_vdev;
    }

    /**
     * Debug:
     *  arceos_hypercall(0x42, 0x77776666, 0xdeadbeef);
     */

    goto end;

err_unregister_vdev:
    misc_deregister(&arceos_vdev);

end:
    return err;
}


module_init(arceos_init);
module_exit(arceos_exit);
