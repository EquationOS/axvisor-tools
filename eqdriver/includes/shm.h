#pragma once

#include <linux/types.h>
#include <uapi/linux/shm.h>

int eq_shmget(key_t key, size_t size, int shmflg);
int eq_shmctl(int shmid, int cmd, struct shmid_ds *buf);

int eq_shmat(struct file *file, struct vm_area_struct *vma, int shmid);