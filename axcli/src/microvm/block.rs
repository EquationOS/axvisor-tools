use std::fs::{File, OpenOptions};
use std::io;
use std::thread;
use std::time::Duration;

use axerrno::{AxResult, ax_err, ax_err_type};
use eqvm_defs::{
    MICROVM_BLOCK_NOTIFY_MAGIC, MICROVM_BLOCK_NOTIFY_PAGE_SIZE, MICROVM_BLOCK_NOTIFY_RING_SIZE,
    MMAP_MICROVM_BLOCK_NOTIFY_MAGIC_NUMBER, MicroVmBlockNotifyPage,
};
use libc::{MAP_FAILED, MAP_SHARED, PROT_READ, PROT_WRITE, mmap};

use crate::microvm::config::BlockDeviceConfig;

const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub struct BlockBackend {
    _worker: thread::JoinHandle<()>,
}

struct BlockDrive {
    config: BlockDeviceConfig,
    _file: File,
}

struct BlockNotifySession {
    ring: *mut MicroVmBlockNotifyPage,
    tail: u32,
}

unsafe impl Send for BlockNotifySession {}

impl BlockNotifySession {
    fn attach(instance_id: usize, instance_fd: i32, ring_gpa: usize) -> AxResult<Self> {
        if ring_gpa == 0 {
            return ax_err!(
                BadState,
                format_args!(
                    "block drives configured for instance {}, but notify ring GPA is zero",
                    instance_id
                )
            );
        }
        let ring = unsafe {
            mmap(
                core::ptr::null_mut(),
                MICROVM_BLOCK_NOTIFY_PAGE_SIZE,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                instance_fd,
                MMAP_MICROVM_BLOCK_NOTIFY_MAGIC_NUMBER << 12,
            )
        };
        if ring == MAP_FAILED {
            return ax_err!(
                BadState,
                format_args!(
                    "failed to map block notify ring for instance {}: {}",
                    instance_id,
                    io::Error::last_os_error()
                )
            );
        }

        let ring = ring as *mut MicroVmBlockNotifyPage;
        let magic = unsafe { (*ring).magic };
        if magic != MICROVM_BLOCK_NOTIFY_MAGIC {
            return ax_err!(
                BadState,
                format_args!(
                    "block notify ring magic mismatch for instance {}: got {:#x}, expected {:#x}",
                    instance_id, magic, MICROVM_BLOCK_NOTIFY_MAGIC
                )
            );
        }
        let tail = unsafe { (*ring).tail.load(std::sync::atomic::Ordering::Acquire) };
        info!(
            "microVM block notify ring attached: instance={} hpa={:#x} ring_size={}",
            instance_id, ring_gpa, MICROVM_BLOCK_NOTIFY_RING_SIZE
        );
        Ok(Self { ring, tail })
    }

    fn poll(&mut self, instance_id: usize) {
        let ring = unsafe { &mut *self.ring };
        let head = ring.head.load(std::sync::atomic::Ordering::Acquire);
        while self.tail != head {
            let idx = (self.tail as usize) % MICROVM_BLOCK_NOTIFY_RING_SIZE;
            let entry = ring.entries[idx];
            debug!(
                "microVM block notify: instance={} queue_id={} seq={} flags={:#x}",
                instance_id, entry.queue_id, entry.seq, entry.flags
            );
            self.tail = self.tail.wrapping_add(1);
        }
        ring.tail
            .store(self.tail, std::sync::atomic::Ordering::Release);
    }
}

pub fn start_block_backend(
    instance_id: usize,
    instance_fd: i32,
    notify_ring_gpa: usize,
    drives: Vec<BlockDeviceConfig>,
) -> AxResult<BlockBackend> {
    let mut opened_drives = Vec::with_capacity(drives.len());
    for drive in drives {
        let mut options = OpenOptions::new();
        options.read(true);
        if !drive.is_read_only {
            options.write(true);
        }
        let file = options.open(&drive.path_on_host).map_err(|e| {
            ax_err_type!(
                InvalidInput,
                format_args!(
                    "failed to open block drive '{}' at '{}': {}",
                    drive.drive_id, drive.path_on_host, e
                )
            )
        })?;
        info!(
            "microVM block drive armed: instance={} drive_id={} path={} root={} readonly={}",
            instance_id,
            drive.drive_id,
            drive.path_on_host,
            drive.is_root_device,
            drive.is_read_only
        );
        opened_drives.push(BlockDrive {
            config: drive,
            _file: file,
        });
    }
    let notify = BlockNotifySession::attach(instance_id, instance_fd, notify_ring_gpa)?;

    let worker_name = format!("eqblk-{}", instance_id);
    let worker = thread::Builder::new()
        .name(worker_name)
        .spawn(move || block_backend_loop(instance_id, instance_fd, opened_drives, notify))
        .map_err(|e| {
            ax_err_type!(
                BadState,
                format_args!("failed to spawn block backend thread: {}", e)
            )
        })?;

    Ok(BlockBackend { _worker: worker })
}

fn block_backend_loop(
    instance_id: usize,
    instance_fd: i32,
    drives: Vec<BlockDrive>,
    mut notify: BlockNotifySession,
) {
    let drive_summary = drives
        .iter()
        .map(|drive| drive.config.drive_id.as_str())
        .collect::<Vec<_>>()
        .join(",");
    info!(
        "microVM split virtio-blk backend running: instance={} fd={} drives={}",
        instance_id, instance_fd, drive_summary
    );

    loop {
        notify.poll(instance_id);
        thread::sleep(IDLE_POLL_INTERVAL);
    }
}
