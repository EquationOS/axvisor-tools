#pragma once

#include <linux/types.h>

enum hvc_fid
{
	HCreateInstance = 0xe0000000 | 2,
	HShmGet = 0xe0000000 | 8,
	HMicroVMBoot = 0xe0000000 | 0x20,
	HMicroVMInjectIrq = 0xe0000000 | 0x21,
	HMicroVMQueryIrqRoute = 0xe0000000 | 0x22,
	HMicroVMSetVcpuCount = 0xe0000000 | 0x25,
	HMicroVMHyperAllocVfioDmaPoll = 0xe0000000 | 0x2c,
	HMicroVMHyperAllocVfioDmaComplete = 0xe0000000 | 0x2d,
	HMicroVMHyperAllocDebugReclaim = 0xe0000000 | 0x2e,
	HMicroVMGuestMemCopy = 0xe0000000 | 0x32,
	HMicroVMGuestRamMmapStateUpdate = 0xe0000000 | 0x33,
	HMicroVMHyperAllocMemoryTarget = 0xe0000000 | 0x34,
	HMicroVMHyperAllocHostQuery = 0xe0000000 | 0x35,
	HMicroVMHyperAllocVfioDmaDebugRequest = 0xe0000000 | 0x36,
	HMicroVMHyperAllocEqGateDrain = 0xe0000000 | 0x37,
	HMicroVMHyperAllocEqGateDebugEnqueue = 0xe0000000 | 0x38,
	HMicroVMGuestRamTranslate = 0xe0000000 | 0x3a,
	HMicroVMGuestRamMmapZapPoll = 0xe0000000 | 0x3b,
	HMicroVMGuestRamMmapZapComplete = 0xe0000000 | 0x3c,
	HMicroVMStop = 0xe0000000 | 0x3e,
};

int hvc_create_instance(
	__u64 instance_type, __u64 mapping_type, __u64 instance_metadata_ptr);

int hvc_shmget(__u64 key, __u64 size, __u64 shmflg, __u64 shm_base_ptr);

int hvc_microvm_boot(
	__u64 instance_id, __u64 entry_point, __u64 boot_protocol);
int hvc_inject_microvm_irq(__u64 instance_id, __u64 msix_index);
int hvc_query_microvm_irq_route(__u64 route_query_ptr);
int hvc_set_microvm_vcpu_count(__u64 instance_id, __u64 vcpu_count);
int hvc_hyperalloc_vfio_dma_poll(__u64 dma_op_ptr);
int hvc_hyperalloc_vfio_dma_complete(__u64 dma_op_ptr);
int hvc_microvm_guest_mem_copy(__u64 copy_arg_ptr);
int hvc_microvm_guest_ram_mmap_state_update(__u64 update_arg_ptr);
int hvc_hyperalloc_memory_target(__u64 instance_id, __u64 req_ptr);
int hvc_hyperalloc_host_query(__u64 instance_id, __u64 query_ptr);
int hvc_hyperalloc_debug_reclaim(__u64 instance_id, __u64 req_ptr);
int hvc_hyperalloc_vfio_dma_debug_request(__u64 instance_id, __u64 dma_op_ptr);
int hvc_hyperalloc_eqgate_drain(__u64 instance_id, __u64 req_ptr);
int hvc_hyperalloc_eqgate_debug_enqueue(__u64 instance_id, __u64 req_ptr);
int hvc_microvm_guest_ram_translate(__u64 translate_arg_ptr);
int hvc_microvm_guest_ram_mmap_zap_poll(__u64 zap_op_ptr);
int hvc_microvm_guest_ram_mmap_zap_complete(__u64 zap_op_ptr);
int hvc_microvm_stop(__u64 instance_id);
