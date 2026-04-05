#pragma once

#include <linux/types.h>

enum hvc_fid
{
	HCreateInstance = 0xe0000000 | 2,
	HShmGet = 0xe0000000 | 8,
	HMmapSync = 0xe0000000 | 12,
};

int hvc_create_instance(
	__u64 instance_type, __u64 mapping_type, __u64 instance_metadata_ptr);

int hvc_shmget(__u64 key, __u64 size, __u64 shmflg, __u64 shm_base_ptr);

__u64 hvc_mmap_sync(
	__u64 va, __u64 size, __u64 pfn, __u64 flags, __u64 instance_id);
