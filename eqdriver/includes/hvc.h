#pragma once

#include <linux/types.h>

enum hvc_fid
{
	HCreateInstance = 0xe0000000 | 2,
	HShmGet = 0xe0000000 | 8,
	HMicroVMInjectIrq = 0xe0000000 | 0x21,
};

int hvc_create_instance(
	__u64 instance_type, __u64 mapping_type, __u64 instance_metadata_ptr);

int hvc_shmget(__u64 key, __u64 size, __u64 shmflg, __u64 shm_base_ptr);

int hvc_inject_microvm_irq(__u64 instance_id, __u64 msix_index);
