#pragma once

#include <linux/types.h>

enum hvc_fid
{
	HCreateInstance = 0xe0000000 | 2,
};

int hvc_create_instance(
	__u64 instance_type, __u64 mapping_type, __u64 scf_queue_base_ptr,
	__u64 scf_queue_size_ptr, __u64 page_cache_base_ptr,
	__u64 page_cache_size_ptr);
