#pragma once

#ifndef __KERNEL__
#include <stdint.h>
#include <sys/ioctl.h>
#endif

#define EQMANAGER_NAME "eqmanager"

#define EQINSTANCE_DEV_PREFIX "eqinstance_"

#define MAX_EQ_INSTANCES_NUM (64)

#define MMAP_SCF_MAGIC_NUMBER (0x45534346) // "ESCF"
#define MMAP_PAGE_CACHE_MAGIC_NUMBER (0x45504350) // "EPCP"

typedef struct eq_create_instance_arg
{
	uint64_t instance_id;	// Instance ID, set by kernel driver.
	uint64_t instance_type; // Instance type
	uint64_t mode;	// EPT fault handling mode, e.g., VMEXIT, VE
} eq_create_instance_arg_t;

typedef struct eq_remove_instance_arg
{
	uint64_t instance_id; // Instance ID
} eq_remove_instance_arg_t;

// int shmget(key_t key, size_t size, int shmflg);
// returns the shared memory ID (shmid) on success, or -1 on failure.
typedef struct eq_shmget_arg
{
	uint64_t key;	// Shared memory Key, passed by user space.
	uint64_t size; // Size of the shared memory region, set by user space.
	uint64_t shmflg; // Shared memory flags, e.g., IPC_CREAT, IPC_EXCL, etc.
} eq_shmget_arg_t;

// These two are performed by mmap/munmap on the fd of "/dev/eqmanager"
// void *shmat(int shmid, const void *shmaddr, int shmflg);
// int shmdt(const void *shmaddr);

// int shmctl(int shmid, int cmd, struct shmid_ds *buf);
typedef struct eq_shmctl_arg
{
	uint64_t shmid; // Shared memory ID, set by kernel driver.
	uint64_t cmd;   // Command, e.g., IPC_RMID to remove the shared memory.
	uint64_t buf;   // Pointer to a structure containing additional information.
} eq_shmctl_arg_t;


#define EQ_CREATE_INSTANCE _IOW(0, 0, eq_create_instance_arg_t)
#define EQ_REMOVE_INSTANCE _IOW(0, 1, eq_remove_instance_arg_t)
#define EQ_SHMGET _IOW(1, 2, eq_shmget_arg_t)
#define EQ_SHMCTL _IOW(1, 3, eq_shmctl_arg_t)