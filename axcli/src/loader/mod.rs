pub(crate) mod elf;

use crate::ExecuteArgs;

fn split_args(junction_args: &[String]) -> (Vec<String>, Vec<String>) {
    // Split the junction_args into two parts:
    // 1. The part before the first "--" (junction arguments)
    // 2. The part after the first "--" (application arguments)
    // If "--" is not present, the second part will be empty.
    if let Some(pos) = junction_args.iter().position(|s| s == "--") {
        let junc = junction_args[..pos].to_vec();
        let app = junction_args[pos + 1..].to_vec();
        (junc, app)
    } else {
        (junction_args.to_vec(), vec![])
    }
}

/// A User-level executor to boot ELF.
/// Just for test and debug purpose.
pub fn local_execute(args: ExecuteArgs) {
    info!("Create APP with args: {:?}", args.exec_args);

    elf::load_junction(&args.exec_args);

    panic!("Should not reach here, local_execute should not return");
}
