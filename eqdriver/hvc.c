#include <linux/types.h>

#include "includes/hvc.h"

int hvc_call(
	__u64 code, __u64 arg1, __u64 arg2, __u64 arg3, __u64 arg4, __u64 arg5,
	__u64 arg6);

/**
 * x86 version of the hypercall function
 * Refer to:
 * <https://github.com/arceos-hypervisor/x86_vcpu/blob/0d73ec10b04e187c6aabd110123a6bf96e2f2ae0/src/vmx/vcpu.rs#L1332>
 */
int hvc_call(
	__u64 code, __u64 arg1, __u64 arg2, __u64 arg3, __u64 arg4, __u64 arg5,
	__u64 arg6)
{
	int result;
	asm volatile("vmcall"
				 : "=a"(result)
				 : "a"(code), "D"(arg1), "S"(arg2), "d"(arg3), "c"(arg4),
				   "r"(arg5), "r"(arg6)
				 : "memory");
	return result;
}

int hvc_create_instance(
	__u64 instance_type, __u64 mapping_type, __u64 shm_base_ptr, __u64 scf_queue_base_ptr)
{
	return hvc_call(
		HCreateInstance, instance_type, mapping_type, shm_base_ptr, scf_queue_base_ptr, 0, 0);
}

int hvc_load_mmap(
	__u64 instance_id, __u64 gva, __u64 gpa, __u64 len, __u64 flags, __u64 prot)
{
	return hvc_call(HLoadMMap, instance_id, gva, gpa, len, flags, prot);
}