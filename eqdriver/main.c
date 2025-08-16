/**
 * @brief A simple Linux kernel module for EquationOS.
 */
#include <linux/version.h>

#include <linux/kernel.h>
#include <linux/module.h>

#include "includes/utils.h"
#include "includes/eqmanager.h"
#include "includes/instance.h"

#define EQUATION_VERSION "0.0.1"

MODULE_AUTHOR("EquationOS Group");
MODULE_LICENSE("GPL v3 | Mulan PSL v2");
MODULE_DESCRIPTION("Management driver for EquationOS");
MODULE_VERSION(EQUATION_VERSION);

static int __init equqtion_init(void)
{
	int ret;

	INFO("Initializing EquationOS kernel driver v%s\n", EQUATION_VERSION);
	ret = init_eqmanagement_device();
	if (ret < 0)
	{
		ERROR("Failed to initialize eqmanagement device\n");
		return ret;
	}
	instances_init();
	return 0;
}

static void __exit equation_exit(void)
{
	instances_exit();
	exit_eqmanagement_device();
	INFO("Exiting Equation driver, welcome back!\n");
}

module_init(equqtion_init);
module_exit(equation_exit);
