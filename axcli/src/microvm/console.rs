use std::io::{self, Read, Write};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use libc::{MAP_FAILED, MAP_SHARED, PROT_READ, PROT_WRITE, mmap};

use eqvm_defs::{
    MICROVM_CONSOLE_RING_DATA_SIZE, MICROVM_CONSOLE_RING_MAGIC, MICROVM_CONSOLE_RING_SIZE,
    MMAP_MICROVM_CONSOLE_MAGIC_NUMBER, MicroVmConsoleRing,
};

use crate::ioctl::EQINSTANCE_DEV_PREFIX;

static CONSOLE_SESSION: OnceLock<Mutex<MicroVmConsoleSession>> = OnceLock::new();

pub struct MicroVmConsoleSession {
    ring: *mut MicroVmConsoleRing,
    guest_to_host_tail: u32,
    host_to_guest_head: u32,
}

unsafe impl Send for MicroVmConsoleSession {}

impl MicroVmConsoleSession {
    pub fn attach(instance_fd: i32, instance_id: usize, ring_gpa: usize) -> Result<(), String> {
        if ring_gpa == 0 {
            return Ok(());
        }
        set_stdin_nonblocking()?;
        let ring = unsafe {
            mmap(
                core::ptr::null_mut(),
                MICROVM_CONSOLE_RING_SIZE,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                instance_fd,
                MMAP_MICROVM_CONSOLE_MAGIC_NUMBER << 12,
            )
        };
        if ring == MAP_FAILED {
            return Err(format!(
                "Failed to map microVM console ring for {}: {}",
                format!("{}{}", EQINSTANCE_DEV_PREFIX, instance_id),
                io::Error::last_os_error()
            ));
        }
        let ring = ring as *mut MicroVmConsoleRing;
        if unsafe { (*ring).magic } != MICROVM_CONSOLE_RING_MAGIC {
            return Err("microVM console ring magic mismatch".to_string());
        }
        let guest_to_host_tail = unsafe {
            (*ring)
                .guest_to_host_tail
                .load(std::sync::atomic::Ordering::Acquire)
        };
        let host_to_guest_head = unsafe {
            (*ring)
                .host_to_guest_head
                .load(std::sync::atomic::Ordering::Acquire)
        };
        let _ = CONSOLE_SESSION.set(Mutex::new(MicroVmConsoleSession {
            ring,
            guest_to_host_tail,
            host_to_guest_head,
        }));
        Ok(())
    }

    pub fn poll(&mut self) {
        self.drain_guest_output();
        self.drain_stdin_input();
    }

    fn drain_guest_output(&mut self) {
        let ring = unsafe { &mut *self.ring };
        let head = ring
            .guest_to_host_head
            .load(std::sync::atomic::Ordering::Acquire);
        while self.guest_to_host_tail != head {
            let idx = (self.guest_to_host_tail as usize) % MICROVM_CONSOLE_RING_DATA_SIZE;
            let byte = ring.guest_to_host[idx];
            let _ = io::stdout().write_all(&[byte]);
            self.guest_to_host_tail = self.guest_to_host_tail.wrapping_add(1);
        }
        ring.guest_to_host_tail.store(
            self.guest_to_host_tail,
            std::sync::atomic::Ordering::Release,
        );
        let _ = io::stdout().flush();
    }

    fn drain_stdin_input(&mut self) {
        let ring = unsafe { &mut *self.ring };
        let mut buf = [0u8; 1];
        let Ok(n) = io::stdin().read(&mut buf) else {
            return;
        };
        if n == 0 {
            return;
        }
        let head = ring
            .host_to_guest_head
            .load(std::sync::atomic::Ordering::Acquire);
        let tail = ring
            .host_to_guest_tail
            .load(std::sync::atomic::Ordering::Acquire);
        let used = head.wrapping_sub(tail) as usize;
        if used >= MICROVM_CONSOLE_RING_DATA_SIZE {
            ring.host_to_guest_dropped
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        let idx = (self.host_to_guest_head as usize) % MICROVM_CONSOLE_RING_DATA_SIZE;
        ring.host_to_guest[idx] = buf[0];
        self.host_to_guest_head = self.host_to_guest_head.wrapping_add(1);
        ring.host_to_guest_head.store(
            self.host_to_guest_head,
            std::sync::atomic::Ordering::Release,
        );
    }
}

fn set_stdin_nonblocking() -> Result<(), String> {
    let flags = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_GETFL, 0) };
    if flags < 0 {
        return Err(format!(
            "Failed to get stdin flags: {}",
            io::Error::last_os_error()
        ));
    }
    let ret = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if ret < 0 {
        return Err(format!(
            "Failed to set stdin nonblocking: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

pub fn attach_console(instance_fd: i32, instance_id: usize, ring_gpa: usize) -> Result<(), String> {
    MicroVmConsoleSession::attach(instance_fd, instance_id, ring_gpa)
}

pub fn poll_console_forever() -> ! {
    loop {
        poll_console_once();
        thread::sleep(Duration::from_millis(10));
    }
}

pub fn poll_console_once() {
    if let Some(session) = CONSOLE_SESSION.get()
        && let Ok(mut session) = session.lock()
    {
        session.poll();
    }
}
