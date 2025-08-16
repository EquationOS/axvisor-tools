pub(crate) mod elf;

use crate::ExecuteArgs;

/// A User-level executor to boot ELF.
/// Just for test and debug purpose.
pub fn local_execute(args: ExecuteArgs) {
    info!("Create APP with args: {:?}", args.exec_args);

    elf::local_execute_app(&args.exec_args);

    panic!("Should not reach here, local_execute should not return");
}
