use core::panic;

use goblin::elf::Elf;
use libc::*;

use linux_libc_auxv::{AuxVar, AuxVarFlags, StackLayoutBuilder, StackLayoutRef};

use equation_defs::{USER_LDSO_BASE_VA, USER_PIE_BASE_VA, USER_STACK_SIZE, USER_STACK_TOP_VA};

// // const STACK_SIZE: usize = 1024 * 1024 * 8;
// const STACK_SIZE: usize = 0x1000 * 4; // 16KB stack size
// const PIE_BASE: usize = 0x40000000;
// const LDSO_BASE: usize = 0x7f0000000000;

unsafe fn mmap_segment(base: usize, ph: &goblin::elf::ProgramHeader, elf_data: &[u8], fd: i32) {
    let vaddr = base + ph.p_vaddr as usize;
    let memsz = ph.p_memsz as usize;
    let filesz = ph.p_filesz as usize;
    let offset = ph.p_offset as usize;

    let prot = (if ph.is_read() { PROT_READ } else { 0 })
        | (if ph.is_write() { PROT_WRITE } else { 0 })
        | (if ph.is_executable() { PROT_EXEC } else { 0 });

    let aligned_addr = vaddr & !0xfff;
    let aligned_offset = offset & !0xfff;
    let end_addr = (vaddr + memsz + 0xfff) & !0xfff;
    let size = end_addr - aligned_addr;

    debug!(
        "[*] Mapping fd {} segment: vaddr={:#x}, prot={:#x}, offset={:#x}, memsz={:#x}, filesz={:#x}",
        fd, vaddr, prot, offset, memsz, filesz
    );
    debug!(
        "[*] Mapping segment: vaddr={:#x}, size={:#x}, offset={:#x}",
        aligned_addr, size, aligned_offset
    );

    let ret = mmap(
        aligned_addr as *mut c_void,
        size,
        (prot | PROT_WRITE) as c_int,
        (MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED) as c_int,
        -1,
        0,
    );
    assert_ne!(ret, MAP_FAILED);

    assert_eq!(
        ret as usize, aligned_addr,
        "mmap returned unexpected address: expected {:#x}, got {:#x}",
        aligned_addr, ret as usize
    );

    std::ptr::copy_nonoverlapping(
        elf_data[offset..offset + filesz].as_ptr(),
        vaddr as *mut u8,
        filesz,
    );

    if memsz > filesz {
        let zero_start = vaddr + filesz;
        let zero_len = memsz - filesz;
        debug!(
            "[*] Zeroing out segment: start={:#x}, length={:#x}",
            zero_start, zero_len
        );
        core::ptr::write_bytes(zero_start as *mut u8, 0, zero_len);
    }
}

unsafe fn mprotect_segment(base: usize, ph: &goblin::elf::ProgramHeader) {
    let vaddr = base + ph.p_vaddr as usize;
    let memsz = ph.p_memsz as usize;
    let filesz = ph.p_filesz as usize;
    let offset = ph.p_offset as usize;

    let prot = (if ph.is_read() { PROT_READ } else { 0 })
        | (if ph.is_write() { PROT_WRITE } else { 0 })
        | (if ph.is_executable() { PROT_EXEC } else { 0 });

    let aligned_addr = vaddr & !0xfff;
    let end_addr = (vaddr + memsz + 0xfff) & !0xfff;
    let size = end_addr - aligned_addr;

    debug!(
        "[*] Protecting segment: vaddr={:#x}, size={:#x}, prot={:#x}, offset={:#x}, memsz={:#x}, filesz={:#x}",
        vaddr, size, prot, offset, memsz, filesz
    );

    let mprotect_ret = mprotect(aligned_addr as *mut c_void, size, prot as c_int);
    assert_eq!(mprotect_ret, 0, "Failed to set memory protection");
}

unsafe fn mmap_elf(path: &str, base: usize) -> (Elf<'static>, usize, Option<String>) {
    info!("[*] Loading ELF: {:?}", path);

    let mut path_string: String = path.into();
    if let Some(pos) = path_string.find('\0') {
        assert_eq!(
            pos,
            path_string.len() - 1,
            "strings must not contain interim NUL bytes"
        );
    }

    if !path_string.ends_with('\0') {
        path_string.push('\0');
    }

    let fd = openat(
        AT_FDCWD,
        path_string.as_ptr() as *const c_char,
        O_RDONLY | O_CLOEXEC,
    );
    assert!(fd >= 2, "Failed to open ELF file: {path}, error code {fd}",);

    let mut stat: stat = unsafe { core::mem::zeroed() };

    let res = fstat(fd, &mut stat);
    assert_eq!(res, 0, "Failed to fstat ELF file: {path}, error {res}");

    let file_length = stat.st_size as usize;

    let data = mmap(
        0 as *mut c_void,
        file_length,
        PROT_READ as c_int,
        MAP_PRIVATE as c_int,
        fd as c_int,
        0,
    );

    assert_ne!(data, MAP_FAILED, "Failed to mmap ELF file: {}", path);
    info!(
        "[*] Mapped ELF file: {} at address {:#x}",
        path, data as usize
    );
    let elf_data = unsafe { core::slice::from_raw_parts(data as *const u8, file_length) };

    let elf = Elf::parse(elf_data).expect("Failed to parse ELF");

    let is_pie = elf.header.e_type == goblin::elf::header::ET_DYN;
    let base = if is_pie { base } else { 0 };

    let mut interp_path = None;

    for ph in elf
        .program_headers
        .iter()
        .filter(|ph| ph.p_type == goblin::elf::program_header::PT_LOAD)
    {
        unsafe { mmap_segment(base, ph, elf_data, fd) };
    }

    if let Some(interp_ph) = elf
        .program_headers
        .iter()
        .find(|ph| ph.p_type == goblin::elf::program_header::PT_INTERP)
    {
        warn!("[*] Found PT_INTERP segment: {:?}", interp_ph);
        let size = interp_ph.p_filesz as usize;

        // Print the interpreter string
        let interp_str = core::str::from_utf8(
            &elf_data[interp_ph.p_offset as usize..interp_ph.p_offset as usize + size - 1],
        )
        .unwrap_or("<invalid>");

        interp_path = Some(String::from(interp_str));
        info!("[*] Interpreter: {:?}", interp_path);
    }

    for ph in elf
        .program_headers
        .iter()
        .filter(|ph| ph.p_type == goblin::elf::program_header::PT_LOAD)
    {
        unsafe { mprotect_segment(base, ph) };
    }

    let res = close(fd);
    assert_eq!(res, 0, "Failed to close ELF file: {path} error {res}");

    println!("[*] Loaded ELF: {:?}", path);
    (elf, base, interp_path)
}

unsafe fn setup_raw_stack() -> *mut c_void {
    let (fd, flags) = (-1, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED);

    let stack = mmap(
        (USER_STACK_TOP_VA - USER_STACK_SIZE) as *mut c_void, // Start of the stack
        USER_STACK_SIZE,
        (PROT_READ | PROT_WRITE) as c_int,
        flags as c_int,
        fd,
        0,
    );
    assert_ne!(stack, MAP_FAILED);
    info!("[*] Allocated raw stack at: {:#p}", stack);
    let stack_top = stack as usize + USER_STACK_SIZE;
    assert_eq!(stack_top, USER_STACK_TOP_VA, "Stack top mismatch");
    stack_top as *mut c_void
}

#[unsafe(no_mangle)]
unsafe fn setup_stack_with_args(
    argv: &Vec<String>,
    envp: &Vec<String>,
    elf: &Elf,
    entry: *mut u8,
    phdr: *mut u8,
    ldso_base: *mut u8,
) -> *mut c_void {
    let (fd, flags) = (-1, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED);

    let stack = mmap(
        (USER_STACK_TOP_VA - USER_STACK_SIZE) as *mut c_void, // Start of the stack
        USER_STACK_SIZE,
        PROT_READ | PROT_WRITE,
        flags,
        fd,
        0,
    );
    assert_ne!(stack, MAP_FAILED);

    let stack_top = stack as usize + USER_STACK_SIZE;
    assert_eq!(stack_top, USER_STACK_TOP_VA);

    let mut stack_builder = StackLayoutBuilder::new();
    for s in argv.iter() {
        stack_builder.add_argv(s.clone());
    }

    for s in envp.iter() {
        stack_builder.add_envv(s.clone());
    }

    let mut random_bytes = [0u8; 16];
    for (index, byte) in random_bytes.iter_mut().enumerate() {
        *byte = index as u8; // Fill with dummy data for now
    }

    warn!("random_bytes: @{:#p}", &random_bytes);

    stack_builder.add_auxv(AuxVar::Pagesz(4096)); // 4KB page size
    stack_builder.add_auxv(AuxVar::Phdr(phdr));
    stack_builder.add_auxv(AuxVar::Phent(elf.header.e_phentsize as usize));
    stack_builder.add_auxv(AuxVar::Phnum(elf.header.e_phnum as usize));
    stack_builder.add_auxv(AuxVar::Entry(entry));
    stack_builder.add_auxv(AuxVar::Flags(AuxVarFlags::NOT_PRESERVE_ARGV0));
    stack_builder.add_auxv(AuxVar::Random(random_bytes));
    stack_builder.add_auxv(AuxVar::Base(ldso_base)); // Base address for ld.so

    let (sp, stack_size) = stack_builder.build_on_stack(stack_top);

    info!("[*] Stack layout: @{:#x}, size: {:#x}", sp, stack_size);

    let layout = StackLayoutRef::new(
        unsafe { core::slice::from_raw_parts_mut(sp as *mut u8, stack_size) },
        None,
    );

    for (i, arg) in unsafe { layout.argv_iter() }.enumerate() {
        info!("  [{i}] {}", arg.to_str().unwrap());
    }
    for (i, env) in unsafe { layout.envv_iter() }.enumerate() {
        info!("  [env {i}] {}", env.to_str().unwrap());
    }
    for auxv in unsafe { layout.auxv_iter() } {
        info!("  [auxv] {:?}", auxv);
    }

    info!(
        "[*] Allocated stack at: {:#p}, sp = {:#x}, stack_top: {:#x}",
        stack, sp, stack_top
    );
    sp as *mut c_void
}

/// Load an ELF application and its interpreter (if any) into local/shared memory.
/// This function will map the ELF segments into memory, set up the stack with arguments and environment variables,
/// and return the entry point and stack pointer.
/// If the ELF is a Position Independent Executable (PIE), it will be mapped at the specified base address.
/// If an interpreter is specified, it will also be mapped and its entry point will be returned.
/// The function will panic if the application path is not provided or if any mapping fails.
///
/// ## Arguments
/// - `args`: A vector of strings containing the application path and its arguments.
/// - `envs`: A vector of strings containing the environment variables.
/// - `fd`: An optional file descriptor for shared memory mapping.
///     - If `None`, anonymous mapping will be used (local execution, for debug purposes).
///     - If `Some(fd)`, the segments will be mapped to the shared memory region
///       associated with the file descriptor (for execution in axvisor).
/// ## Returns
/// - A tuple containing the entry point address and the stack pointer address.
///
pub unsafe fn load_app(args: &Vec<String>, envs: &Vec<String>) -> (usize, usize) {
    if args.is_empty() {
        panic!("No application path provided");
    }

    let app_path = &args[0];

    info!("[*] Loading application: {}", app_path);

    let app_path = args[0].clone();

    let (app_elf, app_base, interp_path) = unsafe { mmap_elf(app_path.as_str(), USER_PIE_BASE_VA) };

    let (entry, stack) = if let Some(interp_path) = interp_path {
        let (ldso_elf, ldso_base, path) = unsafe { mmap_elf(&interp_path, USER_LDSO_BASE_VA) };
        if let Some(path) = path {
            panic!(
                "[*] Found interpreter: {:?} for interp {:?}",
                path, interp_path
            );
        }
        warn!("ldso_base: 0x{:x}, junc_base: 0x{:x}", ldso_base, app_base);

        let is_junc_pie = app_elf.header.e_type == goblin::elf::header::ET_DYN;

        let (entry, phdr) = if is_junc_pie {
            (
                app_base + app_elf.entry as usize,
                app_base + app_elf.header.e_phoff as usize,
            )
        } else {
            (app_elf.entry as usize, app_elf.header.e_phoff as usize)
        };

        let stack = unsafe {
            setup_stack_with_args(
                args,
                envs,
                &app_elf,
                entry as *mut u8,
                phdr as *mut u8,
                ldso_base as *mut u8,
            )
        };
        (ldso_base + ldso_elf.entry as usize, stack as usize)
    } else {
        let stack = unsafe { setup_raw_stack() };

        (app_base + app_elf.entry as usize, stack as usize)
    };
    (entry, stack)
}

pub(super) fn local_execute_app(app_args: &Vec<String>) {
    let envp = vec![];

    let (entry, stack) = unsafe { load_app(app_args, &envp) };

    jumping(entry, stack)
}

#[unsafe(no_mangle)]
fn jumping(entry: usize, stack: usize) -> ! {
    println!("[*] Jumping to entry {:#x}, stack {:#x}", entry, stack);
    unsafe {
        core::arch::asm! {
            "mov rsp, {0}",
            "jmp {1}",
            in(reg) stack,
            in(reg) entry,
            options(noreturn)
        }
    }
}
