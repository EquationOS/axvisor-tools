
#pragma once

#include "eqmanager.h"

int create_instance(eq_create_instance_arg_t *arg);
int remove_instance(int instance_id);

void instances_init(void);
void instances_exit(void);