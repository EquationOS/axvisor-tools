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

pub fn hvc_init_shim() {
    let result = trigger_hypercall(HyperCallCode::HInitShim, 0, 0, 0, 0, 0, 0);
    info!("hvc_init_shim result: {:#x}", result);
}

pub fn hvc_create_instance(
    instance_type: u64,
    mapping_type: u64,
    file_size: u64,
    shared_pages_base: u64,
    shared_pages_num: u64,
) -> isize {
    trigger_hypercall(
        HyperCallCode::HCreateInstance,
        instance_type,
        mapping_type,
        file_size,
        shared_pages_base,
        shared_pages_num,
        0,
    )
}
