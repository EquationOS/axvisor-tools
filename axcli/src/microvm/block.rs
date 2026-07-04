use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::sync::atomic::{AtomicUsize, Ordering, fence};
use std::thread;
use std::time::Duration;

use axerrno::{AxResult, ax_err, ax_err_type};
use eqvm_defs::{
    EQ_MICROVM_GUEST_MEM_COPY_MAX_LEN,
    MICROVM_BLOCK_NOTIFY_MAGIC, MICROVM_BLOCK_NOTIFY_PAGE_SIZE, MICROVM_BLOCK_NOTIFY_RING_SIZE,
    MICROVM_IRQ_INDEX_SOURCE_BLOCK, MMAP_MICROVM_BLOCK_NOTIFY_MAGIC_NUMBER,
    MicroVmBlockNotifyEntry, MicroVmBlockNotifyPage,
};
use libc::{MAP_FAILED, MAP_SHARED, PROT_READ, PROT_WRITE, mmap};

use crate::ioctl;
use crate::microvm::config::BlockDeviceConfig;
use crate::microvm::vstate::memory::{ByteValued, Bytes, GuestAddress, GuestMemoryMmap};

const ACTIVE_POLL_INTERVAL: Duration = Duration::from_micros(100);
const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(1);
const COPY_CHUNK_SIZE: usize = 64 * 1024;
const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;
const VRING_DESC_F_INDIRECT: u16 = 4;
const VIRTIO_MSI_NO_VECTOR: u16 = 0xffff;
const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;
const VIRTIO_BLK_T_FLUSH: u32 = 4;
const VIRTIO_BLK_T_GET_ID: u32 = 8;
const VIRTIO_BLK_S_OK: u8 = 0;
const VIRTIO_BLK_S_IOERR: u8 = 1;
const VIRTIO_BLK_S_UNSUPP: u8 = 2;
const BLOCK_TRACE_LIMIT: usize = 32;

static BLOCK_REQUEST_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);
static BLOCK_COMPLETE_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);
static BLOCK_DRAIN_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);

pub struct BlockBackend {
    _worker: thread::JoinHandle<()>,
}

struct BlockDrive {
    config: BlockDeviceConfig,
    file: File,
    file_len: u64,
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

    fn pop(&mut self, instance_id: usize) -> Option<MicroVmBlockNotifyEntry> {
        let ring = unsafe { &mut *self.ring };
        let head = ring.head.load(std::sync::atomic::Ordering::Acquire);
        if self.tail == head {
            return None;
        }
        let idx = (self.tail as usize) % MICROVM_BLOCK_NOTIFY_RING_SIZE;
        let entry = ring.entries[idx];
        trace!(
            "microVM block notify: instance={} queue_id={} seq={} flags={:#x} size={} msix={} desc={:#x} avail={:#x} used={:#x}",
            instance_id,
            entry.queue_id,
            entry.seq,
            entry.flags,
            entry.queue_size,
            entry.msix_vector,
            entry.desc_addr,
            entry.avail_addr,
            entry.used_addr
        );
        self.tail = self.tail.wrapping_add(1);
        ring.tail
            .store(self.tail, std::sync::atomic::Ordering::Release);
        Some(entry)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VringDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

unsafe impl ByteValued for VringDesc {}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VringUsedElem {
    id: u32,
    len: u32,
}

unsafe impl ByteValued for VringUsedElem {}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioBlkOutHdr {
    type_: u32,
    ioprio: u32,
    sector: u64,
}

unsafe impl ByteValued for VirtioBlkOutHdr {}

#[derive(Clone, Copy, Default)]
struct QueueRuntime {
    queue_size: u16,
    msix_vector: u16,
    desc_addr: u64,
    avail_addr: u64,
    used_addr: u64,
    last_avail_idx: u16,
}

impl QueueRuntime {
    fn configured(&self) -> bool {
        self.queue_size != 0 && self.desc_addr != 0 && self.avail_addr != 0 && self.used_addr != 0
    }
}

enum GuestMemoryAccess {
    #[allow(dead_code)]
    Mmap(GuestMemoryMmap),
    Mediated { instance_id: usize, instance_fd: i32 },
}

impl GuestMemoryAccess {
    fn mediated(instance_id: usize, instance_fd: i32) -> Self {
        Self::Mediated {
            instance_id,
            instance_fd,
        }
    }

    fn read_slice(&self, buf: &mut [u8], addr: u64) -> AxResult {
        match self {
            Self::Mmap(mem) => mem.read_slice(buf, GuestAddress(addr)).map_err(|e| {
                ax_err_type!(
                    BadState,
                    format_args!("guest read_slice {:#x}+{}: {}", addr, buf.len(), e)
                )
            }),
            Self::Mediated {
                instance_id,
                instance_fd,
            } => {
                let chunk_limit = EQ_MICROVM_GUEST_MEM_COPY_MAX_LEN as usize;
                let mut done = 0usize;
                while done < buf.len() {
                    let n = chunk_limit.min(buf.len() - done);
                    ioctl::ioctl_microvm_guest_mem_read(
                        *instance_fd,
                        *instance_id as u64,
                        addr + done as u64,
                        &mut buf[done..done + n],
                    )
                    .map_err(|e| ax_err_type!(BadState, format_args!("{}", e)))?;
                    done += n;
                }
                Ok(())
            }
        }
    }

    fn write_slice(&self, buf: &[u8], addr: u64) -> AxResult {
        match self {
            Self::Mmap(mem) => mem.write_slice(buf, GuestAddress(addr)).map_err(|e| {
                ax_err_type!(
                    BadState,
                    format_args!("guest write_slice {:#x}+{}: {}", addr, buf.len(), e)
                )
            }),
            Self::Mediated {
                instance_id,
                instance_fd,
            } => {
                let chunk_limit = EQ_MICROVM_GUEST_MEM_COPY_MAX_LEN as usize;
                let mut done = 0usize;
                while done < buf.len() {
                    let n = chunk_limit.min(buf.len() - done);
                    ioctl::ioctl_microvm_guest_mem_write(
                        *instance_fd,
                        *instance_id as u64,
                        addr + done as u64,
                        &buf[done..done + n],
                    )
                    .map_err(|e| ax_err_type!(BadState, format_args!("{}", e)))?;
                    done += n;
                }
                Ok(())
            }
        }
    }

    fn read_obj<T: ByteValued>(&self, addr: u64) -> AxResult<T> {
        let mut value = core::mem::MaybeUninit::<T>::uninit();
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(
                value.as_mut_ptr().cast::<u8>(),
                core::mem::size_of::<T>(),
            )
        };
        self.read_slice(bytes, addr)?;
        Ok(unsafe { value.assume_init() })
    }

    fn write_obj<T: ByteValued>(&self, value: T, addr: u64) -> AxResult {
        let bytes = unsafe {
            core::slice::from_raw_parts(
                (&value as *const T).cast::<u8>(),
                core::mem::size_of::<T>(),
            )
        };
        self.write_slice(bytes, addr)
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
        let file_len = file
            .metadata()
            .map_err(|e| {
                ax_err_type!(
                    InvalidInput,
                    format_args!(
                        "failed to stat block drive '{}' at '{}': {}",
                        drive.drive_id, drive.path_on_host, e
                    )
                )
            })?
            .len();
        let lock_mode = lock_drive_file(instance_id, &drive, &file)?;
        info!(
            "microVM block drive armed: instance={} drive_id={} path={} root={} readonly={} bytes={} lock={}",
            instance_id,
            drive.drive_id,
            drive.path_on_host,
            drive.is_root_device,
            drive.is_read_only,
            file_len,
            lock_mode
        );
        opened_drives.push(BlockDrive {
            config: drive,
            file,
            file_len,
        });
    }
    let notify = BlockNotifySession::attach(instance_id, instance_fd, notify_ring_gpa)?;

    let worker_name = format!("eqblk-{}", instance_id);
    let worker = thread::Builder::new()
        .name(worker_name)
        .spawn(move || {
            let guest_memory = GuestMemoryAccess::mediated(instance_id, instance_fd);
            block_backend_loop(
                instance_id,
                instance_fd,
                guest_memory,
                opened_drives,
                notify,
            )
        })
        .map_err(|e| {
            ax_err_type!(
                BadState,
                format_args!("failed to spawn block backend thread: {}", e)
            )
        })?;

    Ok(BlockBackend { _worker: worker })
}

fn lock_drive_file(
    instance_id: usize,
    drive: &BlockDeviceConfig,
    file: &File,
) -> AxResult<&'static str> {
    let (operation, mode) = if drive.is_read_only {
        (libc::LOCK_SH | libc::LOCK_NB, "shared-readonly")
    } else {
        (libc::LOCK_EX | libc::LOCK_NB, "exclusive-writable")
    };
    let ret = unsafe { libc::flock(file.as_raw_fd(), operation) };
    if ret != 0 {
        return ax_err!(
            InvalidInput,
            format_args!(
                "failed to acquire {} lock for block drive '{}' at '{}' (instance={}): {}; use a separate writable rootfs image per eqLinux instance",
                mode,
                drive.drive_id,
                drive.path_on_host,
                instance_id,
                io::Error::last_os_error()
            )
        );
    }
    Ok(mode)
}

fn block_backend_loop(
    instance_id: usize,
    instance_fd: i32,
    guest_memory: GuestMemoryAccess,
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
    if drives.len() != 1 {
        error!(
            "microVM split virtio-blk backend only supports one drive, got {}",
            drives.len()
        );
        return;
    }

    let mut queue = QueueRuntime::default();
    loop {
        let mut active = false;
        while let Some(entry) = notify.pop(instance_id) {
            active = true;
            if let Err(err) = handle_queue_notify(
                instance_id,
                instance_fd,
                &guest_memory,
                &drives[0],
                &mut queue,
                entry,
            ) {
                warn!(
                    "microVM block queue handling failed: instance={} queue={} seq={} err={:?}",
                    instance_id, entry.queue_id, entry.seq, err
                );
            }
        }
        if queue.configured() {
            match drain_queue_available(
                instance_id,
                instance_fd,
                &guest_memory,
                &drives[0],
                &mut queue,
                "poll",
            ) {
                Ok(completed) => {
                    if completed != 0 {
                        active = true;
                    }
                }
                Err(err) => {
                    warn!(
                        "microVM block queue polling failed: instance={} err={:?}",
                        instance_id, err
                    );
                }
            }
        }
        thread::sleep(if active {
            ACTIVE_POLL_INTERVAL
        } else {
            IDLE_POLL_INTERVAL
        });
    }
}

fn read_u16(mem: &GuestMemoryAccess, addr: u64) -> AxResult<u16> {
    let mut buf = [0u8; 2];
    mem.read_slice(&mut buf, addr)
        .map_err(|e| ax_err_type!(BadState, format_args!("guest read_u16 {:#x}: {}", addr, e)))?;
    Ok(u16::from_le_bytes(buf))
}

fn write_u16(mem: &GuestMemoryAccess, addr: u64, val: u16) -> AxResult {
    mem.write_slice(&val.to_le_bytes(), addr)
        .map_err(|e| ax_err_type!(BadState, format_args!("guest write_u16 {:#x}: {}", addr, e)))
}

fn write_status(mem: &GuestMemoryAccess, desc: VringDesc, status: u8) -> AxResult {
    if desc.len == 0 || (desc.flags & VRING_DESC_F_WRITE) == 0 {
        return ax_err!(InvalidInput, "invalid virtio-blk status descriptor");
    }
    mem.write_slice(&[status], desc.addr)
        .map_err(|e| ax_err_type!(BadState, format_args!("guest status write: {}", e)))
}

fn read_desc(mem: &GuestMemoryAccess, queue: &QueueRuntime, index: u16) -> AxResult<VringDesc> {
    if index >= queue.queue_size {
        return ax_err!(
            InvalidInput,
            format_args!(
                "descriptor index {} exceeds queue size {}",
                index, queue.queue_size
            )
        );
    }
    let addr = queue.desc_addr + (index as u64) * core::mem::size_of::<VringDesc>() as u64;
    mem.read_obj::<VringDesc>(addr)
        .map_err(|e| ax_err_type!(BadState, format_args!("guest desc read {:#x}: {}", addr, e)))
}

fn descriptor_chain(
    mem: &GuestMemoryAccess,
    queue: &QueueRuntime,
    head: u16,
) -> AxResult<Vec<VringDesc>> {
    let mut chain = Vec::new();
    let mut index = head;
    for _ in 0..queue.queue_size {
        let desc = read_desc(mem, queue, index)?;
        if (desc.flags & VRING_DESC_F_INDIRECT) != 0 {
            return ax_err!(
                Unsupported,
                "indirect virtio-blk descriptors are not negotiated"
            );
        }
        chain.push(desc);
        if (desc.flags & VRING_DESC_F_NEXT) == 0 {
            return Ok(chain);
        }
        index = desc.next;
    }
    ax_err!(InvalidInput, "virtio-blk descriptor chain loop")
}

fn read_request_header(mem: &GuestMemoryAccess, desc: VringDesc) -> AxResult<VirtioBlkOutHdr> {
    if desc.len < core::mem::size_of::<VirtioBlkOutHdr>() as u32 {
        return ax_err!(InvalidInput, "virtio-blk header descriptor is too small");
    }
    if (desc.flags & VRING_DESC_F_WRITE) != 0 {
        return ax_err!(
            InvalidInput,
            "virtio-blk header descriptor is device-writable"
        );
    }
    mem.read_obj::<VirtioBlkOutHdr>(desc.addr)
        .map_err(|e| {
            ax_err_type!(
                BadState,
                format_args!("guest virtio-blk header read {:#x}: {}", desc.addr, e)
            )
        })
}

fn request_data_len(descs: &[VringDesc]) -> AxResult<u64> {
    descs.iter().try_fold(0u64, |acc, desc| {
        acc.checked_add(desc.len as u64)
            .ok_or_else(|| ax_err_type!(InvalidInput, "virtio-blk request data length overflow"))
    })
}

fn check_image_range(drive: &BlockDrive, file_off: u64, len: u64) -> AxResult {
    let end = file_off.checked_add(len).ok_or_else(|| {
        ax_err_type!(
            InvalidInput,
            format_args!(
                "virtio-blk image offset overflow: off={} len={}",
                file_off, len
            )
        )
    })?;
    if end > drive.file_len {
        return ax_err!(
            InvalidInput,
            format_args!(
                "virtio-blk request exceeds backing image: drive={} off={} len={} image_len={}",
                drive.config.drive_id, file_off, len, drive.file_len
            )
        );
    }
    Ok(())
}

fn copy_file_to_guest(
    file: &File,
    mem: &GuestMemoryAccess,
    mut file_off: u64,
    desc: VringDesc,
) -> AxResult<usize> {
    if (desc.flags & VRING_DESC_F_WRITE) == 0 {
        return ax_err!(
            InvalidInput,
            "read request data descriptor is not device-writable"
        );
    }
    let mut remaining = desc.len as usize;
    let mut guest_off = desc.addr;
    let mut copied = 0usize;
    let mut buf = vec![0u8; COPY_CHUNK_SIZE.min(remaining.max(1))];
    while remaining != 0 {
        let n = COPY_CHUNK_SIZE.min(remaining);
        buf[..n].fill(0);
        let mut read_total = 0usize;
        while read_total < n {
            match file.read_at(&mut buf[read_total..n], file_off + read_total as u64) {
                Ok(0) => break,
                Ok(bytes) => read_total += bytes,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return ax_err!(Io, format_args!("block image read: {}", e)),
            }
        }
        mem.write_slice(&buf[..n], guest_off)
            .map_err(|e| {
                ax_err_type!(
                    BadState,
                    format_args!("guest data write {:#x}+{}: {}", guest_off, n, e)
                )
            })?;
        remaining -= n;
        guest_off += n as u64;
        file_off += n as u64;
        copied += n;
    }
    Ok(copied)
}

fn copy_guest_to_file(
    mem: &GuestMemoryAccess,
    file: &File,
    mut file_off: u64,
    desc: VringDesc,
) -> AxResult<usize> {
    if (desc.flags & VRING_DESC_F_WRITE) != 0 {
        return ax_err!(
            InvalidInput,
            "write request data descriptor is device-writable"
        );
    }
    let mut remaining = desc.len as usize;
    let mut guest_off = desc.addr;
    let mut copied = 0usize;
    let mut buf = vec![0u8; COPY_CHUNK_SIZE.min(remaining.max(1))];
    while remaining != 0 {
        let n = COPY_CHUNK_SIZE.min(remaining);
        mem.read_slice(&mut buf[..n], guest_off)
            .map_err(|e| {
                ax_err_type!(
                    BadState,
                    format_args!("guest data read {:#x}+{}: {}", guest_off, n, e)
                )
            })?;
        let mut written = 0usize;
        while written < n {
            match file.write_at(&buf[written..n], file_off + written as u64) {
                Ok(0) => return ax_err!(Io, "zero-length block image write"),
                Ok(bytes) => written += bytes,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return ax_err!(Io, format_args!("block image write: {}", e)),
            }
        }
        remaining -= n;
        guest_off += n as u64;
        file_off += n as u64;
        copied += n;
    }
    Ok(copied)
}

fn copy_id_to_guest(mem: &GuestMemoryAccess, desc: VringDesc) -> AxResult<usize> {
    if (desc.flags & VRING_DESC_F_WRITE) == 0 {
        return ax_err!(InvalidInput, "get-id descriptor is not device-writable");
    }
    let id = b"eqvisor-rootfs\0";
    let mut buf = vec![0u8; desc.len as usize];
    let copy_len = id.len().min(buf.len());
    buf[..copy_len].copy_from_slice(&id[..copy_len]);
    mem.write_slice(&buf, desc.addr)
        .map_err(|e| ax_err_type!(BadState, format_args!("guest get-id write: {}", e)))?;
    Ok(buf.len())
}

fn process_request(
    mem: &GuestMemoryAccess,
    drive: &BlockDrive,
    queue: &QueueRuntime,
    head: u16,
) -> AxResult<u32> {
    let chain = descriptor_chain(mem, queue, head)?;
    if chain.len() < 2 {
        return ax_err!(InvalidInput, "virtio-blk descriptor chain is too short");
    }
    let header = read_request_header(mem, chain[0])?;
    let status_desc = *chain
        .last()
        .ok_or_else(|| ax_err_type!(InvalidInput, "missing virtio-blk status descriptor"))?;
    let data_descs = &chain[1..chain.len() - 1];
    let request_len = request_data_len(data_descs)?;
    let mut status = VIRTIO_BLK_S_OK;
    let mut written_len = 0usize;
    let mut file_off = header.sector.checked_mul(512).ok_or_else(|| {
        ax_err_type!(
            InvalidInput,
            format_args!("virtio-blk sector overflow: {}", header.sector)
        )
    })?;

    let result = match header.type_ {
        VIRTIO_BLK_T_IN => {
            check_image_range(drive, file_off, request_len)?;
            for desc in data_descs {
                written_len += copy_file_to_guest(&drive.file, mem, file_off, *desc)?;
                file_off += desc.len as u64;
            }
            Ok(())
        }
        VIRTIO_BLK_T_OUT => {
            if drive.config.is_read_only {
                status = VIRTIO_BLK_S_IOERR;
                Ok(())
            } else {
                check_image_range(drive, file_off, request_len)?;
                for desc in data_descs {
                    let copied = copy_guest_to_file(mem, &drive.file, file_off, *desc)?;
                    file_off += copied as u64;
                }
                Ok(())
            }
        }
        VIRTIO_BLK_T_FLUSH => drive
            .file
            .sync_data()
            .map_err(|e| ax_err_type!(Io, format_args!("block image flush: {}", e))),
        VIRTIO_BLK_T_GET_ID => {
            for desc in data_descs {
                written_len += copy_id_to_guest(mem, *desc)?;
            }
            Ok(())
        }
        _ => {
            status = VIRTIO_BLK_S_UNSUPP;
            Ok(())
        }
    };
    if let Err(err) = result {
        warn!(
            "virtio-blk request failed: drive={} type={} sector={} err={:?}",
            drive.config.drive_id, header.type_, header.sector, err
        );
        status = VIRTIO_BLK_S_IOERR;
    }
    write_status(mem, status_desc, status)?;
    written_len += 1;
    let trace_id = BLOCK_REQUEST_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
    if trace_id < BLOCK_TRACE_LIMIT {
        trace!(
            "microVM block request complete: drive={} head={} type={} sector={} data_len={} chain={} status={} used_len={}",
            drive.config.drive_id,
            head,
            header.type_,
            header.sector,
            request_len,
            chain.len(),
            status,
            written_len
        );
    }
    Ok(written_len as u32)
}

fn add_used(mem: &GuestMemoryAccess, queue: &QueueRuntime, head: u16, len: u32) -> AxResult {
    let used_idx = read_u16(mem, queue.used_addr + 2)?;
    let elem = VringUsedElem {
        id: head as u32,
        len,
    };
    let elem_addr = queue.used_addr + 4 + ((used_idx % queue.queue_size) as u64) * 8;
    mem.write_obj(elem, elem_addr).map_err(|e| {
        ax_err_type!(
            BadState,
            format_args!("guest used elem write {:#x}: {}", elem_addr, e)
        )
    })?;
    write_u16(mem, queue.used_addr + 2, used_idx.wrapping_add(1))
}

fn drain_queue_available(
    instance_id: usize,
    instance_fd: i32,
    mem: &GuestMemoryAccess,
    drive: &BlockDrive,
    queue: &mut QueueRuntime,
    reason: &'static str,
) -> AxResult<usize> {
    if !queue.configured() {
        return Ok(0);
    }

    fence(Ordering::Acquire);
    let avail_flags = read_u16(mem, queue.avail_addr)?;
    let avail_idx = read_u16(mem, queue.avail_addr + 2)?;
    let pending = queue.last_avail_idx != avail_idx;
    if pending {
        let drain_trace_id = BLOCK_DRAIN_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
        if drain_trace_id < BLOCK_TRACE_LIMIT {
            trace!(
                "microVM block queue drain: instance={} reason={} last_avail={} avail_idx={} avail_flags={:#x} msix={}",
                instance_id,
                reason,
                queue.last_avail_idx,
                avail_idx,
                avail_flags,
                queue.msix_vector
            );
        }
    }

    let mut completed = 0usize;
    while queue.last_avail_idx != avail_idx {
        let ring_addr =
            queue.avail_addr + 4 + ((queue.last_avail_idx % queue.queue_size) as u64) * 2;
        let head = read_u16(mem, ring_addr)?;
        let used_len = process_request(mem, drive, queue, head)?;
        add_used(mem, queue, head, used_len)?;
        queue.last_avail_idx = queue.last_avail_idx.wrapping_add(1);
        completed += 1;
    }

    if completed == 0 {
        return Ok(0);
    }

    fence(Ordering::Release);
    let trace_id = BLOCK_COMPLETE_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
    if trace_id < BLOCK_TRACE_LIMIT {
        trace!(
            "microVM block queue completed: instance={} reason={} completed={} used_idx={} msix={}",
            instance_id,
            reason,
            completed,
            read_u16(mem, queue.used_addr + 2).unwrap_or(0),
            queue.msix_vector
        );
    }

    if queue.msix_vector != VIRTIO_MSI_NO_VECTOR {
        let encoded_msix_index = MICROVM_IRQ_INDEX_SOURCE_BLOCK | queue.msix_vector as u32;
        ioctl::ioctl_inject_instance_irq(instance_fd, instance_id as u64, encoded_msix_index)
            .map_err(|e| ax_err_type!(BadState, format_args!("{}", e)))?;
        if trace_id < BLOCK_TRACE_LIMIT {
            trace!(
                "microVM block IRQ injected: instance={} msix={} encoded_msix={:#x} completed={}",
                instance_id, queue.msix_vector, encoded_msix_index, completed
            );
        }
    } else {
        warn!(
            "microVM block completed without MSI-X vector: instance={} completed={}",
            instance_id, completed
        );
    }

    Ok(completed)
}

fn handle_queue_notify(
    instance_id: usize,
    instance_fd: i32,
    mem: &GuestMemoryAccess,
    drive: &BlockDrive,
    queue: &mut QueueRuntime,
    entry: MicroVmBlockNotifyEntry,
) -> AxResult {
    if entry.queue_id != 0 {
        return ax_err!(
            Unsupported,
            format_args!("unsupported virtio-blk queue {}", entry.queue_id)
        );
    }
    if entry.queue_size == 0
        || entry.desc_addr == 0
        || entry.avail_addr == 0
        || entry.used_addr == 0
    {
        return ax_err!(InvalidInput, "incomplete virtio-blk queue notify metadata");
    }
    if entry.queue_size > 256 || !entry.queue_size.is_power_of_two() {
        return ax_err!(
            InvalidInput,
            format_args!("invalid virtio-blk queue size {}", entry.queue_size)
        );
    }
    if queue.queue_size != entry.queue_size
        || queue.desc_addr != entry.desc_addr
        || queue.avail_addr != entry.avail_addr
        || queue.used_addr != entry.used_addr
    {
        *queue = QueueRuntime {
            queue_size: entry.queue_size,
            msix_vector: entry.msix_vector,
            desc_addr: entry.desc_addr,
            avail_addr: entry.avail_addr,
            used_addr: entry.used_addr,
            last_avail_idx: 0,
        };
        info!(
            "microVM block queue configured: instance={} q={} size={} desc={:#x} avail={:#x} used={:#x} msix={}",
            instance_id,
            entry.queue_id,
            entry.queue_size,
            entry.desc_addr,
            entry.avail_addr,
            entry.used_addr,
            entry.msix_vector
        );
    } else {
        queue.msix_vector = entry.msix_vector;
    }

    let _ = drain_queue_available(instance_id, instance_fd, mem, drive, queue, "notify")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_block_path(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos();
        path.push(format!(
            "eqvisor-axcli-block-lock-test-{}-{}-{}",
            name,
            std::process::id(),
            suffix
        ));
        path
    }

    fn block_drive(path: &std::path::Path, readonly: bool) -> BlockDeviceConfig {
        BlockDeviceConfig {
            drive_id: "rootfs".to_string(),
            path_on_host: path.to_string_lossy().to_string(),
            is_root_device: true,
            is_read_only: readonly,
            cache_type: None,
            io_engine: None,
        }
    }

    #[test]
    fn drive_lock_allows_shared_readonly_openers() {
        let path = temp_block_path("shared-readonly");
        std::fs::write(&path, vec![0u8; 512]).expect("create backing file");
        let first = File::open(&path).expect("open first readonly backing file");
        let second = File::open(&path).expect("open second readonly backing file");
        let drive = block_drive(&path, true);

        assert!(lock_drive_file(1, &drive, &first).is_ok());
        assert!(lock_drive_file(2, &drive, &second).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn drive_lock_rejects_second_writable_opener() {
        let path = temp_block_path("exclusive-writable");
        std::fs::write(&path, vec![0u8; 512]).expect("create backing file");
        let first = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open first writable backing file");
        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open second writable backing file");
        let drive = block_drive(&path, false);

        assert!(lock_drive_file(1, &drive, &first).is_ok());
        assert!(lock_drive_file(2, &drive, &second).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
