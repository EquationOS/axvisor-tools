use core::panic;
use std::collections::HashSet;
use std::ffi::CString;
use std::fmt::Debug;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::str::FromStr;

use goblin::elf::Elf;
use libc::*;

use linux_libc_auxv::{AuxVar, AuxVarFlags, StackLayoutBuilder, StackLayoutRef};

const STACK_SIZE: usize = 1024 * 1024 * 8;
const PIE_BASE: usize = 0x40000000;
const LDSO_BASE: usize = 0x7f0000000000;
const LIB_BASE_START: usize = 0x6000000000;

unsafe fn mmap_segment(base: usize, ph: &goblin::elf::ProgramHeader, data: &[u8]) {
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
        "[*] Mapping segment: vaddr={:#x}, size={:#x}, prot={:#x}, offset={:#x}, memsz={:#x}, filesz={:#x}",
        vaddr, size, prot, offset, memsz, filesz
    );

    let ret = mmap(
        aligned_addr as *mut c_void,
        size,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED,
        -1,
        0,
    );
    assert_ne!(ret, MAP_FAILED);

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
) -> (Elf<'static>, usize, &'static [u8], Option<PathBuf>) {
    info!("[*] Loading ELF: {:?}", path);
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
        mmap_segment(base, ph, static_ref);
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

        // info!(
        //     "[*] Clearing PT_INTERP segment at 0x{:x}, size {}",
        //     interp_addr, size
        // );
        // std::ptr::write_bytes(interp_addr as *mut u8, 0, size);
        // warn!(
        //     "[*] Cleared PT_INTERP segment at 0x{:x}, size {}",
        //     interp_addr, size
        // );
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

fn find_needed_libs(elf: &Elf) -> Vec<String> {
    let mut needed = vec![];
    if let Some(dynamic) = &elf.dynamic {
        let strtab = &elf.dynstrtab;
        for dyn_entry in dynamic
            .dyns
            .iter()
            .filter(|d| d.d_tag == goblin::elf::dynamic::DT_NEEDED)
        {
            let name = strtab.get_at(dyn_entry.d_val as usize).unwrap_or("unknown");
            needed.push(name.to_string());
        }
    }
    warn!("[*] Found needed libraries: {:?}", needed);
    needed
}

unsafe fn load_needed_recursively(
    elf: &Elf,
    base_dir: &str,
    loaded: &mut HashSet<String>,
    mut base_addr: usize,
) {
    for lib in find_needed_libs(elf) {
        if loaded.contains(&lib) {
            continue;
        }
        let path = format!("{}/{}", base_dir, lib);
        info!("[*] Loading needed library: {}", path);
        let (lib_elf, _, _, _) = mmap_elf(&path, base_addr);
        info!("[*] Loaded DT_NEEDED: {}", lib);
        loaded.insert(lib.clone());
        base_addr += 0x2000000; // Bump base per lib
        load_needed_recursively(&lib_elf, base_dir, loaded, base_addr);
    }
}

#[unsafe(no_mangle)]
unsafe fn setup_stack(
    argv: Vec<CString>,
    envp: Vec<CString>,
    elf_base: Option<usize>,
    elf: &Elf,
    ldso_base: usize,
) -> *mut c_void {
    let stack = mmap(
        0x60000_0000 as *mut c_void, // Start of the stack
        STACK_SIZE,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0,
    );
    assert_ne!(stack, MAP_FAILED);

    let stack_top = stack as usize + STACK_SIZE;

    let mut stack_builder = StackLayoutBuilder::new();
    for s in argv.iter() {
        stack_builder.add_argv(s.clone().into_string().expect("Invalid CString"));
    }

    for s in envp.iter() {
        stack_builder.add_envv(s.clone().into_string().expect("Invalid CString"));
    }

    let (entry, phdr) = if let Some(elf_base) = elf_base {
        (
            elf_base + elf.entry as usize,
            elf_base + elf.header.e_phoff as usize,
        )
    } else {
        (elf.entry as usize, elf.header.e_phoff as usize)
    };

    let mut random_bytes = [0u8; 16];
    for (index, byte) in random_bytes.iter_mut().enumerate() {
        *byte = index as u8; // Fill with dummy data for now
    }

    stack_builder.add_auxv(AuxVar::Pagesz(4096)); // 4KB page size
    stack_builder.add_auxv(AuxVar::Phdr(phdr as *mut u8));
    stack_builder.add_auxv(AuxVar::Phent(elf.header.e_phentsize as usize));
    stack_builder.add_auxv(AuxVar::Phnum(elf.header.e_phnum as usize));
    stack_builder.add_auxv(AuxVar::Entry(entry as *mut u8));
    stack_builder.add_auxv(AuxVar::Flags(AuxVarFlags::NOT_PRESERVE_ARGV0));
    stack_builder.add_auxv(AuxVar::Random(random_bytes));
    stack_builder.add_auxv(AuxVar::Base(ldso_base as *mut u8)); // Base address for ld.so

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

pub(super) fn load_junction(junc_args: &[String], app_args: &[String]) {
    let argv_full = {
        let mut v = vec![];
        v.extend(junc_args.iter().map(|s| CString::new(s.as_str()).unwrap()));
        if !app_args.is_empty() {
            v.push(CString::new("--").unwrap());
            v.extend(app_args.iter().map(|s| CString::new(s.as_str()).unwrap()));
        }
        v
    };

    // let envp = vec![CString::new("LD_LIBRARY_PATH=glibc").unwrap()];
    let envp = vec![];

    let junc_path = junc_args[0].clone();

    unsafe {
        let (junc_elf, junc_base, _, interp_path) = mmap_elf(junc_path.as_str(), PIE_BASE);

        let (ldso_elf, ldso_base) = if let Some(interp_path) = interp_path {
            let (ldso_elf, ldso_base, _, path) = mmap_elf(&interp_path, LDSO_BASE);
            if let Some(path) = path {
                panic!(
                    "[*] Found interpreter: {:?} for interp {:?}",
                    path, interp_path
                );
            }
            (ldso_elf, ldso_base)
        } else {
            panic!("[*] No interpreter found in junction ELF");
        };

        warn!("ldso_base: 0x{:x}, junc_base: 0x{:x}", ldso_base, junc_base);

        let is_junc_pie = junc_elf.header.e_type == goblin::elf::header::ET_DYN;
        let junc_base = if is_junc_pie { Some(junc_base) } else { None };

        // 🔁 自动加载所有 DT_NEEDED 依赖库
        // let mut loaded_set = HashSet::new();
        // load_needed_recursively(
        //     &junc_elf,
        //     "/lib/x86_64-linux-gnu/",
        //     &mut loaded_set,
        //     LIB_BASE_START,
        // );

        let stack = setup_stack(argv_full, envp, junc_base, &junc_elf, ldso_base);
        let ld_entry = ldso_base + ldso_elf.entry as usize;

        println!(
            "[*] Jumping to ld.so entry: 0x{:x}, stack {:#x}",
            ld_entry, stack as usize
        );

        core::arch::asm! {
            "mov rsp, {0}",
            "jmp {1}",
            in(reg) stack,
            in(reg) ld_entry,
            options(noreturn)
        }
    }
}
