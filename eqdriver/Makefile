# Makefile for building the kernel module

# Kernel build directory
KDIR ?= /lib/modules/$(shell uname -r)/build

# Current path
CURRENT_PATH := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

# Extra compiler flags
EXTRA_CFLAGS ?=
ccflags-y    += $(EXTRA_CFLAGS) -I $(CURRENT_PATH)/include/

# Module name
MODULE := eqdriver

# Source files
SRC := eqmanager.o \
	   hvc.o \
	   instance.o \
	   shm_region.o \
	   main.o 

# Default target
all:
	$(MAKE) -C $(KDIR) M=$(CURRENT_PATH) modules

# Clean target
clean:
	$(MAKE) -C $(KDIR) M=$(CURRENT_PATH) clean

# Install target
install:
	@sudo cp $(MODULE).ko /lib/modules/$(shell uname -r)/kernel/drivers/
	@sudo depmod -a

# Uninstall target
uninstall:
	@sudo rm -f /lib/modules/$(shell uname -r)/kernel/drivers/$(MODULE).ko
	@sudo depmod -a

# Module build rules
obj-m := eqdriver.o
eqdriver-objs := $(SRC)
