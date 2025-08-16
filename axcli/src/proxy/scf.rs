use lazyinit::LazyInit;
use libc::{c_void, mmap, MAP_FAILED, MAP_FIXED, MAP_SHARED, PROT_READ, PROT_WRITE};

use equation_defs::scf::{ScfDescriptor, SyscallQueueBufferMetadata, SCF_QUEUE_BUFF_MAGIC};
use equation_defs::{SCF_QUEUE_REGION_BASE_VA, SCF_QUEUE_REGION_SIZE};

static mut SYSCALL_QUEUE_BUFFER: LazyInit<SyscallQueueBuffer> = LazyInit::new();

pub struct SyscallQueueBuffer {
    capacity_mask: u16,
    req_index_last: u16,
    rsp_index_shadow: u16,

    meta: &'static mut SyscallQueueBufferMetadata,
    desc: &'static mut [ScfDescriptor],
    req_ring: &'static mut [u16],
    rsp_ring: &'static mut [u16],
}

impl SyscallQueueBuffer {
    pub(super) fn get() -> &'static mut Self {
        unsafe { &mut SYSCALL_QUEUE_BUFFER }
    }

    fn has_request(&self) -> bool {
        self.req_index_last != self.meta.req_index()
    }

    pub(super) fn pop_syscall_request(&mut self) -> Option<(usize, &mut ScfDescriptor)> {
        // spin_lock(&buf->meta->lock);
        if !self.has_request() {
            // spin_unlock(&buf->meta->lock);
            return None;
        }

        let index = self.req_ring[(self.req_index_last & self.capacity_mask) as usize] as usize;
        let desc = &mut self.desc[index];
        self.req_index_last += 1;

        // spin_unlock(&buf->meta->lock);

        Some((index, desc))
    }

    pub(super) fn push_syscall_response(&mut self, index: usize, ret_val: u64) -> bool {
        if index > self.capacity_mask as usize {
            return false; // Invalid index
        }
        // spin_lock(&buf->meta->lock);

        self.desc[index].set_return_value(ret_val);

        self.rsp_ring[(self.rsp_index_shadow & self.capacity_mask) as usize] = index as u16;
        self.rsp_index_shadow += 1;

        // __sync_synchronize();

        // Update the response index in the metadata
        self.meta.set_rsp_index(self.rsp_index_shadow);

        // spin_unlock(&buf->meta->lock);

        true
    }
}

/// Setup the sysycall queue buffer for SCF (system call forwarding).
/// This function maps the syscall queue buffer into this daemon process's address space,
/// Panic if the global `INSTANCE_FD` is not set.
pub fn setup_syscall_proxy_queue_buffer() {
    let instance_fd = super::get_instance_fd();

    let syscall_queue_base = unsafe {
        mmap(
            SCF_QUEUE_REGION_BASE_VA as *mut c_void,
            SCF_QUEUE_REGION_SIZE,
            PROT_READ | PROT_WRITE,
            MAP_SHARED | MAP_FIXED,
            instance_fd,
            // Offset in bytes
            // Use as magic number to tell the kernel driver
            // this is a syscall queue buffer
            (SCF_QUEUE_BUFF_MAGIC as i64) << 12,
        )
    };
    assert_ne!(
        syscall_queue_base,
        MAP_FAILED,
        "Failed to map syscall queue buffer: {}",
        std::io::Error::last_os_error()
    );
    debug!(
        "Mapped syscall queue buffer at {:#p}, size: {:#x}",
        syscall_queue_base, SCF_QUEUE_REGION_SIZE
    );

    let meta = SyscallQueueBufferMetadata::construct_mut();

    while !meta.is_valid() {
        // Wait for the metadata to be valid
        warn!("Syscall queue buffer metadata is not valid, retrying...");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    // Check if the magic number is valid.
    // This may trigger a page_fault if the memory is not mapped correctly.
    assert!(
        meta.is_valid(),
        "Invalid magic number for syscall queue buffer"
    );

    let desc = meta.descriptor_table();
    let req_ring = meta.request_ring();
    let rsp_ring = meta.response_ring();

    info!(
        "Mapped syscall queue buffer at {:#p}, size: {:#x}, capacity: {}",
        syscall_queue_base,
        SCF_QUEUE_REGION_BASE_VA,
        meta.capacity()
    );

    unsafe {
        SYSCALL_QUEUE_BUFFER.init_once(SyscallQueueBuffer {
            capacity_mask: meta.capacity() - 1,
            req_index_last: 0,
            rsp_index_shadow: meta.rsp_index(),
            meta,
            desc,
            req_ring,
            rsp_ring,
        });
    }
}
