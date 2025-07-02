use axerrno::{LinuxError, LinuxResult};

use equation_defs::scf::Sysno;

use crate::proxy::SyscallQueueBuffer;

pub fn poll() {
    let scf = SyscallQueueBuffer::get();
    loop {
        if let Some((index, desc)) = scf.pop_syscall_request() {
            let ret = handle_syscall(desc.sysno(), desc.args());
            let ans = ret.unwrap_or_else(|err| -err.code() as _);

            if !scf.push_syscall_response(index, ans as u64) {
                error!("Failed to push syscall response {index}");
            }
        } else {
            // No request available, sleep or yield to avoid busy waiting
            std::thread::yield_now();
        }
    }
}

fn handle_syscall(syscall_id: Sysno, args: &[u64; 6]) -> LinuxResult<isize> {
    info!("Handling syscall: {:?}, args: {:x?}", syscall_id, args);
    match syscall_id {
        // Handle specific syscalls here
        _ => {
            error!("Unhandled syscall ID: {:?}", syscall_id);
            return Err(LinuxError::ENOSYS);
        }
    }
}
