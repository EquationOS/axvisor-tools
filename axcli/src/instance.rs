use crate::elf::parse_elf_file;
use crate::hvc::hvc_init_shim;
use crate::InstanceCreateArgs;

pub fn create_instance(args: InstanceCreateArgs) {
    info!("Create instance with ELF path: {}", args.elf_path);
    parse_elf_file(&args.elf_path, args.one2onemapping);
}

pub fn init_shim() {
    info!("Init shim");
    hvc_init_shim();
}
