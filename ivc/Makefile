# Makefile for building components of the IVC (Inter-VM Communication) library: kernel driver, user
# library, python bindings, and demo applications.

# Basic information
LIB_NAME 		:= ivc

# Architecture and cross-compilation settings
ARCH 			?= arm64
CROSS 			?= aarch64-linux-musl-
CROSS_KO 		?= aarch64-linux-gnu-
CCOMP 			?= $(CROSS)gcc
CAR 			?= $(CROSS)ar

# Input, output, and kernel dependency directories
MAKEFILE_DIR 	:= $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

BUILD_DIR 		?= $(MAKEFILE_DIR)build
INCLUDE_DIR 	:= $(MAKEFILE_DIR)include
KO_DIR 			:= $(MAKEFILE_DIR)kernel_driver
KO_OUTPUT 		:= axvisor.ko
KDIR 			?= /lib/modules/$(shell uname -r)/build
ULIB_SRC_DIR 	:= $(MAKEFILE_DIR)ulib
DEMO_SRC_DIR 	:= $(MAKEFILE_DIR)demo

# Source files and outputs
ULIB_SRC 		:= $(wildcard $(ULIB_SRC_DIR)/*.c)
ULIB_OBJ 		:= $(patsubst $(ULIB_SRC_DIR)/%.c,$(BUILD_DIR)/ulib/%.o,$(ULIB_SRC))
ULIB_STATIC		:= $(BUILD_DIR)/lib$(LIB_NAME).a
ULIB_SHARED		:= $(BUILD_DIR)/lib$(LIB_NAME).so

DEMO_SRC 		:= $(wildcard $(DEMO_SRC_DIR)/*.c)
DEMO_BIN 		:= $(patsubst $(DEMO_SRC_DIR)/%.c,$(BUILD_DIR)/demo/%, $(DEMO_SRC))


# Compiler flags
CFLAGS 			?= -I$(INCLUDE_DIR)
KO_CFLAGS 		?= $(CFLAGS)
ULIB_CFLAGS 	?= $(CFLAGS) -Wall -Wextra -Os -s -Wl,--gc-sections -fPIC
DEMO_CFLAGS 	?= $(CFLAGS) -Wall -Wextra -Os -s -Wl,--gc-sections -static

.PHONY: all ko ulib demo output_dir clean rebuild
all: ko ulib demo

ko: output_dir
	$(MAKE) -C $(KO_DIR) CROSS_COMPILE=$(CROSS_KO) EXTRA_CFLAGS="$(CFLAGS)" ARCH=$(ARCH)
	cp $(KO_DIR)/$(KO_OUTPUT) $(BUILD_DIR)/

ulib: output_dir $(ULIB_STATIC) $(ULIB_SHARED)
demo: output_dir $(DEMO_BIN)

$(ULIB_STATIC): $(ULIB_OBJ)
	$(CAR) rcs $@ $^

$(ULIB_SHARED): $(ULIB_OBJ)
	$(CCOMP) -shared -o $@ $^

$(BUILD_DIR)/ulib/%.o: $(ULIB_SRC_DIR)/%.c
	$(CCOMP) $(ULIB_CFLAGS) -c $< -o $@

$(BUILD_DIR)/demo/%: $(DEMO_SRC_DIR)/%.c $(ULIB_STATIC)
	$(CCOMP) $(DEMO_CFLAGS) -o $@ $< -L$(BUILD_DIR) -l$(LIB_NAME)

output_dir:
	@mkdir -p $(BUILD_DIR)/ulib
	@mkdir -p $(BUILD_DIR)/demo

clean:
	$(MAKE) -C $(KO_DIR) CROSS_COMPILE=$(CROSS_KO) EXTRA_CFLAGS="$(CFLAGS)" ARCH=$(ARCH) clean
	rm -rf $(BUILD_DIR)

rebuild: clean all
