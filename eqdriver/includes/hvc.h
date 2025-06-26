#pragma once

#include <linux/types.h>

enum hvc_fid
{
	HypervisorDisable = 0,
	HyperVisorPrepareDisable = 1,
	HyperVisorDebug = 2,
	HDebug = 0xe0000000 | 0,
	HCreateInstance = 0xe0000000 | 1,
	HExitProcess = 0xe0000000 | 2,
	HShutdownInstance = 0xe0000000 | 3,
	HMMAP = 0xe0000000 | 4,
	HClone = 0xe0000000 | 5,
	HInitShim = 0xe0000000 | 6,
	HRead = 0xe0000000 | 0x11,
	HWrite = 0xe0000000 | 0x12,
};

int hvc_call(
	__u64 code, __u64 arg1, __u64 arg2, __u64 arg3, __u64 arg4, __u64 arg5,
	__u64 arg6);

int hvc_create_instance(__u64 instance_type, __u64 mapping_type);