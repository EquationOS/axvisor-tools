# Makefile for building the kernel module

CROSS_COMPILER ?= aarch64-linux-gnu-
ARCH ?= arm64

# Kernel build directory
KDIR ?= /lib/modules/$(shell uname -r)/build

# Error messages from gcc will be muted if we omit an explicit definition to CC.
CC := ${CROSS_COMPILE}gcc

CURRENT_PATH := $(shell pwd)

# Module name
MODULE := axvisor-driver

# Source files
SRC := $(MODULE).c

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
obj-m := axvisor.o
