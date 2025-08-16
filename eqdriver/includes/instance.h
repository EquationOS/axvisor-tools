
#pragma once

#include "eqmanager.h"

#define STATUS_CREATED (0)
#define STATUS_SETTING_UP (1)
#define STATUS_RUNNING (2)
#define STATUS_STOPPED (3)

int create_instance(eq_create_instance_arg_t *arg);
int remove_instance(int instance_id);

void instances_init(void);
void instances_exit(void);