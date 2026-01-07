use std::arch::asm;

use axhvc::HyperCallCode;

#[inline(always)]
fn trigger_hypercall(
    code: HyperCallCode,
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    arg5: u64,
    arg6: u64,
) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "vmcall",
            in("rax") code as u64,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("rcx") arg4,
            in("r8") arg5,
            in("r9") arg6,
            lateout("rax") result,
            options(nostack, preserves_flags)
        );
    }
    result
}

#[allow(unused)]
pub fn hvc_debug() {
    let result = trigger_hypercall(HyperCallCode::HDebug, 0, 0, 0, 0, 0, 0);
    info!("hvc_debug result: {:#x}", result);
}

pub fn hvc_init_shim(ksched_shm_base: u64) {
    let result = trigger_hypercall(HyperCallCode::HInitShim, ksched_shm_base, 0, 0, 0, 0, 0);
    info!("hvc_init_shim result: {:#x}", result);
}

pub fn hvc_setup_instance(
    instance_id: u64,
    file_size: u64,
    shared_pages_base: u64,
    shared_pages_num: u64,
) -> isize {
    info!(
        "[*] Setting up instance ID: {}, file size: {}, shared pages base: {:#x}, num: {}",
        instance_id, file_size, shared_pages_base, shared_pages_num
    );

    trigger_hypercall(
        HyperCallCode::HSetupInstance,
        instance_id,
        file_size,
        shared_pages_base,
        shared_pages_num,
        0,
        0,
    )
}

pub fn hvc_daemon_shmat(
    instance_id: u64,
    process_id: u64,
    shmkey: u64,
    shmaddr: u64,
    shmsize: u64,
    shmflg: u64,
) -> isize {
    debug!(
        "[*] Attaching shm instance ID:{}, process ID: {}, key: {:#x}, addr: {:#x}, size: {:#x}, flags: {:#x}",
        instance_id, process_id, shmkey, shmaddr, shmsize, shmflg
    );
    trigger_hypercall(
        HyperCallCode::HIVCSHMAt,
        instance_id,
        process_id,
        shmkey,
        shmaddr,
        shmsize,
        shmflg,
    )
}

pub fn hvc_microvm_boot(instance_id: u64) -> isize {
    info!("[*] Booting microVM instance ID: {}", instance_id);

    trigger_hypercall(HyperCallCode::HMicroVMBoot, instance_id, 0, 0, 0, 0, 0)
}
