ARCH 			?= arm64
CROSS 			?= aarch64-linux-musl-
CROSS_KO 		?= aarch64-linux-gnu-

CCOMP 			?= $(CROSS)gcc
CAR 			?= $(CROSS)ar

MAKEFILE_DIR 	:= $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

BUILD_DIR 		?= $(MAKEFILE_DIR)build
INCLUDE_DIR 	:= $(MAKEFILE_DIR)include
KO_DIR 			:= $(MAKEFILE_DIR)kernel_driver
KDIR 			?= /lib/modules/$(shell uname -r)/build
USER_SRC_DIR 	:= $(MAKEFILE_DIR)user
DEMO_SRC_DIR 	:= $(MAKEFILE_DIR)demo

USER_SRC 		:= $(wildcard $(USER_SRC_DIR)/*.c)
DEMO_SRC 		:= $(wildcard $(DEMO_SRC_DIR)/*.c)
USER_OBJ 		:= $(patsubst $(USER_SRC_DIR)/%.c,$(BUILD_DIR)/user/%.o,$(USER_SRC))
DEMO_BIN 		:= $(patsubst $(DEMO_SRC_DIR)/%.c,$(BUILD_DIR)/demo/%, $(DEMO_SRC))

LIB_NAME 		:= ivc
USER_LIB_STATIC	:= $(BUILD_DIR)/lib$(LIB_NAME).a
USER_LIB_SHARED	:= $(BUILD_DIR)/lib$(LIB_NAME).so

CFLAGS 			?= -I$(INCLUDE_DIR)
KO_CFLAGS 		?= $(CFLAGS)
USER_CFLAGS 	?= $(CFLAGS) -Wall -Wextra -Os -s -Wl,--gc-sections
DEMO_CFLAGS 	?= $(CFLAGS) -Wall -Wextra -Os -s -Wl,--gc-sections -static # 

.PHONY: all ko user user_dyn demo output_dir clean rebuild
all: ko user user_dyn demo

ko:
	$(MAKE) -C $(KO_DIR) CROSS_COMPILE=$(CROSS_KO) EXTRA_CFLAGS="$(CFLAGS)" ARCH=$(ARCH)

user: output_dir $(USER_LIB_STATIC)
user_dyn: output_dir $(USER_LIB_SHARED)
demo: user $(DEMO_BIN)

$(USER_LIB_STATIC): $(USER_OBJ)
	$(CAR) rcs $@ $^

$(USER_LIB_SHARED): $(USER_OBJ)
	$(CCOMP) -shared -o $@ $^

$(BUILD_DIR)/user/%.o: $(USER_SRC_DIR)/%.c
	$(CCOMP) $(USER_CFLAGS) -c $< -o $@

$(BUILD_DIR)/demo/%: $(DEMO_SRC_DIR)/%.c
	$(CCOMP) $(DEMO_CFLAGS) -o $@ $< -L$(BUILD_DIR) -l$(LIB_NAME)

output_dir:
	@mkdir -p $(BUILD_DIR)/user
	@mkdir -p $(BUILD_DIR)/demo

clean:
	$(MAKE) -C $(KO_DIR) CROSS_COMPILE=$(CROSS_KO) EXTRA_CFLAGS="$(CFLAGS)" ARCH=$(ARCH) clean
	rm -rf $(BUILD_DIR)

rebuild: clean all