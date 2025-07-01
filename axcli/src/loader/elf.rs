use core::panic;
use std::fmt::Debug;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use goblin::elf::Elf;
use libc::*;

use linux_libc_auxv::{AuxVar, AuxVarFlags, StackLayoutBuilder, StackLayoutRef};

// const STACK_SIZE: usize = 1024 * 1024 * 8;
const STACK_SIZE: usize = 0x1000 * 4; // 16KB stack size
const PIE_BASE: usize = 0x40000000;
const LDSO_BASE: usize = 0x7f0000000000;

unsafe fn mmap_segment(base: usize, ph: &goblin::elf::ProgramHeader, data: &[u8], fd: Option<i32>) {
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

    let (fd, flags) = if let Some(fd) = fd {
        (fd, MAP_SHARED | MAP_FIXED)
    } else {
        // If no file descriptor is provided, use -1 for anonymous mapping
        (-1, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED)
    };

    trace!(
        "[*] Mapping fd {} segment: vaddr={:#x}, size={:#x}, prot={:#x}, offset={:#x}, memsz={:#x}, filesz={:#x}", 
        fd, vaddr, size, prot, offset, memsz, filesz
    );

    let ret = mmap(
        aligned_addr as *mut c_void,
        size,
        PROT_READ | PROT_WRITE,
        flags,
        fd,
        0,
    );
    assert_ne!(ret, MAP_FAILED);

    if ret as usize == 0 {
        info!("This is exactly what we want, mmap returned 0");
        return;
    } else {
        info!("Get ret from mmap: {:#p}", ret);
    }

    std::ptr::copy_nonoverlapping(
        data[offset..offset + filesz].as_ptr(),
        vaddr as *mut u8,
        filesz,
    );
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

    trace!(
        "[*] Protecting segment: vaddr={:#x}, size={:#x}, prot={:#x}, offset={:#x}, memsz={:#x}, filesz={:#x}",
        vaddr, size, prot, offset, memsz, filesz
    );

    let mprotect_ret = mprotect(aligned_addr as *mut c_void, size, prot);
    assert_eq!(mprotect_ret, 0, "Failed to set memory protection");
}

unsafe fn mmap_elf<P: AsRef<Path> + Debug>(
    path: P,
    base: usize,
    fd: Option<i32>,
) -> (Elf<'static>, usize, &'static [u8], Option<PathBuf>) {
    info!("[*] Loading ELF: {:?}, fd {:?}", path, fd);
    let mut file = File::open(&path).expect("Failed to open ELF");
    let mut data = Vec::new();
    file.read_to_end(&mut data).unwrap();
    let boxed = data.into_boxed_slice();
    let static_ref = Box::leak(boxed);
    let elf = Elf::parse(static_ref).expect("Failed to parse ELF");

    let is_pie = elf.header.e_type == goblin::elf::header::ET_DYN;
    let base = if is_pie { base } else { 0 };

    let mut interp_path = None;

    for ph in elf
        .program_headers
        .iter()
        .filter(|ph| ph.p_type == goblin::elf::program_header::PT_LOAD)
    {
        mmap_segment(base, ph, static_ref, fd);
    }

    if let Some(interp_ph) = elf
        .program_headers
        .iter()
        .find(|ph| ph.p_type == goblin::elf::program_header::PT_INTERP)
    {
        warn!("[*] Found PT_INTERP segment: {:?}", interp_ph);
        let interp_addr = base + interp_ph.p_vaddr as usize;
        let size = interp_ph.p_filesz as usize;

        // Print the interpreter string
        let interp_str = std::str::from_utf8(
            &static_ref[interp_ph.p_offset as usize..interp_ph.p_offset as usize + size - 1],
        )
        .unwrap_or("<invalid>");

        interp_path = Some(PathBuf::from_str(interp_str).unwrap());

        // interp_path = Some(interp_str.to_string());
        info!("[*] Interpreter: {:?}", interp_path);

        info!(
            "[*] Clearing PT_INTERP segment at 0x{:x}, size {}",
            interp_addr, size
        );
        std::ptr::write_bytes(interp_addr as *mut u8, 0, size);
        warn!(
            "[*] Cleared PT_INTERP segment at 0x{:x}, size {}",
            interp_addr, size
        );
    }

    for ph in elf
        .program_headers
        .iter()
        .filter(|ph| ph.p_type == goblin::elf::program_header::PT_LOAD)
    {
        mprotect_segment(base, ph);
    }

    println!("[*] Loaded ELF: {:?}", path);
    (elf, base, static_ref, interp_path)
}

unsafe fn setup_raw_stack(fd: Option<i32>) -> *mut c_void {
    let (fd, flags) = if let Some(fd) = fd {
        (fd, MAP_SHARED | MAP_FIXED)
    } else {
        // If no file descriptor is provided, use -1 for anonymous mapping
        (-1, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED)
    };

    let stack = mmap(
        0x60000_0000 as *mut c_void, // Start of the stack
        STACK_SIZE,
        PROT_READ | PROT_WRITE,
        flags,
        fd,
        0,
    );
    assert_ne!(stack, MAP_FAILED);
    info!("[*] Allocated raw stack at: {:#p}", stack);
    let stack_top = stack as usize + STACK_SIZE;
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
    fd: Option<i32>,
) -> *mut c_void {
    let (fd, flags) = if let Some(fd) = fd {
        (fd, MAP_SHARED | MAP_FIXED)
    } else {
        // If no file descriptor is provided, use -1 for anonymous mapping
        (-1, MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED)
    };

    let stack = mmap(
        0x60000_0000 as *mut c_void, // Start of the stack
        STACK_SIZE,
        PROT_READ | PROT_WRITE,
        flags,
        fd,
        0,
    );
    assert_ne!(stack, MAP_FAILED);

    let stack_top = stack as usize + STACK_SIZE;

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
        println!("  [{i}] {}", arg.to_str().unwrap());
    }
    for (i, env) in unsafe { layout.envv_iter() }.enumerate() {
        println!("  [env {i}] {}", env.to_str().unwrap());
    }
    for auxv in unsafe { layout.auxv_iter() } {
        println!("  [auxv] {:?}", auxv);
    }

    info!(
        "[*] Allocated stack at: {:#p}, sp = {:#x}, stack_top: {:#x}",
        stack, sp, stack_top
    );
    sp as *mut c_void
}

pub unsafe fn load_app(args: &Vec<String>, envs: &Vec<String>, fd: Option<i32>) -> (usize, usize) {
    if args.is_empty() {
        panic!("No application path provided");
    }

    let app_path = &args[0];

    info!("[*] Loading application: {}", app_path);

    let app_path = args[0].clone();

    let (app_elf, app_base, _, interp_path) = unsafe { mmap_elf(app_path.as_str(), PIE_BASE, fd) };

    let (entry, stack) = if let Some(interp_path) = interp_path {
        let (ldso_elf, ldso_base, _, path) = unsafe { mmap_elf(&interp_path, LDSO_BASE, fd) };
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
                fd,
            )
        };
        (ldso_base + ldso_elf.entry as usize, stack as usize)
    } else {
        let stack = unsafe { setup_raw_stack(fd) };

        (app_base + app_elf.entry as usize, stack as usize)
    };
    (entry, stack)
}

pub(super) fn execute_app(app_args: &Vec<String>) {
    let envp = vec![];

    let (entry, stack) = unsafe { load_app(app_args, &envp, None) };

    println!("[*] Jumping to entry: 0x{:x}, stack {:#x}", entry, stack);
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
