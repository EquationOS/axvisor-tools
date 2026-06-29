use std::fs::{File, OpenOptions};
use std::thread;
use std::time::Duration;

use axerrno::{AxResult, ax_err_type};

use crate::microvm::config::BlockDeviceConfig;

const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub struct BlockBackend {
    _worker: thread::JoinHandle<()>,
}

struct BlockDrive {
    config: BlockDeviceConfig,
    _file: File,
}

pub fn start_block_backend(
    instance_id: usize,
    instance_fd: i32,
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

    let worker_name = format!("eqblk-{}", instance_id);
    let worker = thread::Builder::new()
        .name(worker_name)
        .spawn(move || block_backend_loop(instance_id, instance_fd, opened_drives))
        .map_err(|e| {
            ax_err_type!(
                BadState,
                format_args!("failed to spawn block backend thread: {}", e)
            )
        })?;

    Ok(BlockBackend { _worker: worker })
}

fn block_backend_loop(instance_id: usize, instance_fd: i32, drives: Vec<BlockDrive>) {
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
        thread::sleep(IDLE_POLL_INTERVAL);
    }
}
