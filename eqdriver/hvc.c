#include <linux/types.h>

#include "includes/hvc.h"

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

int hvc_create_instance(__u64 instance_type, __u64 mapping_type)
{
	return hvc_call(HCreateInstance, instance_type, mapping_type, 0, 0, 0, 0);
}