use axerrno::{LinuxError, LinuxResult};

use equation_defs::purple_text;
use equation_defs::scf::Sysno;

use crate::proxy::scf::SyscallQueueBuffer;
use crate::proxy::syscall::{fs, misc, net, shm};

pub fn poll() {
    let scf = SyscallQueueBuffer::get();
    loop {
        if let Some((index, desc)) = scf.pop_syscall_request() {
            let ret = handle_syscall(desc.sysno(), desc.args());
            let ans = ret.unwrap_or_else(|err| -err.code() as _);
            if !scf.push_syscall_response(index, ans as u64) {
                error!("Failed to push syscall response {index}");
            }
        } else if scf.should_exit() {
            info!(
                "Instance [{}] SCF daemon exiting as requested",
                super::instance_id()
            );
            break;
        } else {
            // No request available, sleep or yield to avoid busy waiting
            std::hint::spin_loop();
        }
    }
}

macro_rules! color_text {
    ($text:expr, $color:expr) => {{
        format_args!("\x1b[{}m{}\x1b[0m", $color, $text)
    }};
}

fn handle_syscall(syscall_id: Sysno, args: &[u64; 6]) -> LinuxResult<u64> {
    trace!(
        "===Handling syscall: {:?}, args: {:x?}",
        purple_text!(format_args!("{syscall_id:?}")),
        args
    );
    let res = match syscall_id {
        // Handle specific syscalls here
        Sysno::write => fs::proxy_write(args[0], args[1], args[2]),
        Sysno::read => fs::proxy_read(args[0], args[1], args[2]),
        Sysno::access => fs::proxy_access(args[0], args[1]),
        Sysno::openat => fs::proxy_openat_with_stat(args[0], args[1], args[2], args[3], args[4]),
        Sysno::fstat => fs::proxy_fstat(args[0], args[1]),
        Sysno::newfstatat => fs::proxy_newfstatat(args[0], args[1], args[2], args[3]),
        Sysno::statx => fs::proxy_statx(args[0], args[1], args[2], args[3], args[4]),
        Sysno::close => fs::proxy_close(args[0]),
        Sysno::statfs => fs::proxy_statfs(args[0], args[1]),
        Sysno::getdents64 => fs::proxy_getdents64(args[0], args[1], args[2]),
        Sysno::readlinkat => fs::proxy_readlinkat(args[0], args[1], args[2], args[3]),
        Sysno::readlink => fs::proxy_readlink(args[0], args[1], args[2]),
        Sysno::getcwd => fs::proxy_getcwd(args[0], args[1]),
        Sysno::mmap => {
            use memory_addr::PAGE_SIZE_4K;

            let fd = args[4] as i32;
            let offset = args[5] as usize;

            match offset % PAGE_SIZE_4K {
                0 => fs::proxy_mmap_into_pagecache(
                    args[0], args[1], args[2], args[3], args[4], args[5],
                ),
                // Since the offset should be aligned to 4K,
                // if offset % 4K == 1, it means that the caller wants to mmap to a `memfd`.
                // see `shim/src/mm/mmap.rs` for details.
                1 => {
                    shm::sys_mmap_to_memfd(args[0], args[1], args[2], args[3], args[4], args[5] - 1)
                }
                _ => {
                    error!("mmap with unaligned offset: {:#x}, fd: {}.", offset, fd);
                    return Err(LinuxError::EINVAL);
                }
            }
        }
        Sysno::pread64 => fs::proxy_pread64(args[0], args[1], args[2], args[3]),
        Sysno::getrandom => misc::sys_getrandom(args[0], args[1], args[2]),
        Sysno::shmget => shm::sys_shmget(args[0], args[1], args[2]),
        Sysno::shmat => {
            shm::sys_shmat_with_shmget_args(args[0] as i32, args[1], args[2] as i32, args[3])
        }
        Sysno::shmdt => shm::sys_shmdt(args[0]),
        Sysno::socket => net::proxy_socket(args[0], args[1], args[2]),
        Sysno::connect => net::proxy_connect(args[0], args[1], args[2]),
        Sysno::arch_prctl => misc::sys_arch_prctl(args[0], args[1]),
        Sysno::sendmsg => net::proxy_sendmsg(args[0], args[1], args[2]),
        Sysno::recvmsg => net::proxy_recvmsg(args[0], args[1], args[2]),
        Sysno::memfd_create => shm::sys_create_memfd(args[0], args[1]),
        Sysno::ftruncate => fs::proxy_ftruncate(args[0], args[1]),
        Sysno::truncate => fs::proxy_truncate(args[0], args[1]),
        _ => {
            error!("Unhandled syscall ID: {:?}", syscall_id);
            return Err(LinuxError::ENOSYS);
        }
    };

    trace!(
        "===Syscall: {:?} returned: {:?}",
        purple_text!(format_args!("{syscall_id:?}")),
        res
    );

    res
}
