use crate::elf::parse_elf_file;
use crate::InstanceInitArgs;

pub fn init_instance(args: InstanceInitArgs) {
    info!("Initializing instance with ELF path: {}", args.elf_path);
    parse_elf_file(&args.elf_path, args.one2onemapping);
}
