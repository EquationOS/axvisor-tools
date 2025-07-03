use axerrno::{LinuxError, LinuxResult};

use equation_defs::scf::Sysno;

use crate::proxy::fs;
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
            std::hint::spin_loop();
        }
    }
}

fn handle_syscall(syscall_id: Sysno, args: &[u64; 6]) -> LinuxResult<u64> {
    warn!("Handling syscall: {:?}, args: {:x?}", syscall_id, args);
    match syscall_id {
        // Handle specific syscalls here
        Sysno::write => fs::proxy_write(args[0], args[1], args[2]),
        Sysno::read => fs::proxy_read(args[0], args[1], args[2]),
        Sysno::access => fs::proxy_access(args[0], args[1]),
        Sysno::openat => fs::proxy_openat(args[0], args[1], args[2], args[3]),
        Sysno::fstat => fs::proxy_fstat(args[0], args[1]),
        Sysno::close => fs::proxy_close(args[0]),
        Sysno::mmap => fs::proxy_mmap(args[0], args[1], args[2], args[3], args[4], args[5]),
        Sysno::pread64 => fs::proxy_pread64(args[0], args[1], args[2], args[3]),
        _ => {
            error!("Unhandled syscall ID: {:?}", syscall_id);
            return Err(LinuxError::ENOSYS);
        }
    }
}
