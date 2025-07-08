#pragma once

#include <linux/types.h>

enum hvc_fid
{
	HCreateInstance = 0xe0000000 | 2,
	HLoadMMap = 0xe0000000 | 3,
	HSetupInstance = 0xe0000000 | 4,
	HSyncPageCacheRegion = 0xe0000000 | 7,
};

int hvc_create_instance(
	__u64 instance_type, __u64 mapping_type, __u64 pg_pool_base_ptr,
	__u64 pg_pool_size_ptr, __u64 scf_queue_base_ptr, __u64 scf_queue_size_ptr);
int hvc_load_mmap(
	__u64 instance_id, __u64 gva, __u64 gpa, __u64 len, __u64 flags,
	__u64 prot);
int hvc_sync_page_cache_region(
	__u64 instance_id, __u64 gva, __u64 gpa, __u64 len, __u64 flags,
	__u64 prot);
int hvc_setup_instance(__u64 instance_id, __u64 entry, __u64 stack);