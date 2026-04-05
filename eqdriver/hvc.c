#include <linux/types.h>

#include "includes/hvc.h"

__u64 hvc_call(
	__u64 code, __u64 arg1, __u64 arg2, __u64 arg3, __u64 arg4, __u64 arg5,
	__u64 arg6);

/**
 * x86 version of the hypercall function
 * Refer to:
 * <https://github.com/arceos-hypervisor/x86_vcpu/blob/0d73ec10b04e187c6aabd110123a6bf96e2f2ae0/src/vmx/vcpu.rs#L1332>
 */
__u64 hvc_call(
	__u64 code, __u64 arg1, __u64 arg2, __u64 arg3, __u64 arg4, __u64 arg5,
	__u64 arg6)
{
	__u64 result;
	asm volatile("vmcall"
				 : "=a"(result)
				 : "a"(code), "D"(arg1), "S"(arg2), "d"(arg3), "c"(arg4),
				   "r"(arg5), "r"(arg6)
				 : "memory");
	return result;
}

int hvc_create_instance(
	__u64 instance_type, __u64 mode, __u64 instance_metadata_ptr)
{
	return (int)hvc_call(
		HCreateInstance, instance_type, mode, instance_metadata_ptr, 0, 0, 0);
}

int hvc_shmget(__u64 key, __u64 size, __u64 shmflg, __u64 shm_base_ptr)
{
	return (int)hvc_call(HShmGet, key, size, shmflg, shm_base_ptr, 0, 0);
}

__u64 hvc_mmap_sync(
	__u64 va, __u64 size, __u64 pfn, __u64 flags, __u64 instance_id)
{
	return hvc_call(HMmapSync, va, size, pfn, flags, instance_id, 0);
}