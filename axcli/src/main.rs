//! CommandLine Interface and host daemon process for Equation OS.

mod hvc;
mod instance;
mod shared_pages;

#[allow(static_mut_refs)]
mod proxy;

#[allow(non_camel_case_types)]
#[allow(unused)]
mod ioctl;

/// Just for test and debug purpose.
/// A User-level executor to boot ELF.
mod loader;

mod microvm;

mod utils;

use clap::{Args, Parser, Subcommand};

#[macro_use]
extern crate log;

#[derive(Parser, Debug)]
#[command(name = "axcli")]
#[command(about = "CommandLine Interface for AxVisor", long_about = None)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
struct CLI {
    #[command(subcommand)]
    subcmd: CLISubCmd,
}

#[derive(Subcommand, Debug)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
enum CLISubCmd {
    /// Subcommands related to hypervisor itself.
    Hv {
        #[command(subcommand)]
        subcmd: HvSubCmd,
    },
    /// Subcommands related to the management of the container instance.
    Instance {
        #[command(subcommand)]
        subcmd: InstanceSubCmd,
    },
    /// Subcommands related to microVM management.
    Microvm {
        #[command(subcommand)]
        subcmd: MicroVMSubCmd,
    },
    /// Subcommands related to the a local test loader.
    Loader(ExecuteArgs),
}

#[derive(Subcommand, Debug)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
enum HvSubCmd {
    /// Enable arceos-hypervisor type1.5.
    Enable,
    /// Disable arceos-hypervisor type1.5.
    Disable,
}

#[derive(Subcommand, Debug)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
enum InstanceSubCmd {
    /// list the info of the instance
    List,
    /// Init instance runtime environment.
    Init,
    /// Execute a instance by ELF file alone with its arguments.
    Execute(ExecuteArgs),
    Remove {
        /// Instance ID to remove.
        #[arg(short, long)]
        instance_id: i32,
    },
}

#[derive(Subcommand, Debug)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
enum MicroVMSubCmd {
    /// Create a new instance.
    Create(MicroVMCreateArgs),
}

#[derive(Debug, Args)]
struct MicroVMCreateArgs {
    /// Path to the configuration file in json format.
    #[arg(short, long)]
    pub config_file: String,
}

#[derive(Parser, Debug)]
#[command(trailing_var_arg = true)]
struct ExecuteArgs {
    #[arg(required = true)]
    exec_args: Vec<String>,
}

use libc::{SA_SIGINFO, SIGBUS, c_void, sigaction, siginfo_t};
use std::ptr;

extern "C" fn sigbus_handler(_sig: i32, info: *mut siginfo_t, context: *mut c_void) {
    unsafe {
        let ucontext = &*(context as *mut libc::ucontext_t);
        let pc = ucontext.uc_mcontext.gregs[libc::REG_RIP as usize];
        eprintln!(
            "Caught SIGBUS at address: {:?}, program counter: 0x{:x}",
            (*info).si_addr(),
            pc
        );
        std::process::exit(1);
    }
}

extern "C" fn sigsegv_handler(_sig: i32, info: *mut siginfo_t, context: *mut c_void) {
    unsafe {
        let ucontext = &*(context as *mut libc::ucontext_t);
        let pc = ucontext.uc_mcontext.gregs[libc::REG_RIP as usize];
        eprintln!(
            "Caught SIGSEGV at address: {:?}, program counter: 0x{:x}",
            (*info).si_addr(),
            pc
        );
        std::process::exit(1);
    }
}

fn install_signal_handlers() {
    unsafe {
        let mut sa: sigaction = std::mem::zeroed();

        // Install SIGBUS handler
        sa.sa_sigaction = sigbus_handler as usize;
        sa.sa_flags = SA_SIGINFO;
        sigaction(SIGBUS, &sa, ptr::null_mut());

        // Install SIGSEGV handler
        sa = std::mem::zeroed();
        sa.sa_sigaction = sigsegv_handler as usize;
        sa.sa_flags = SA_SIGINFO;
        sigaction(libc::SIGSEGV, &sa, ptr::null_mut());
    }
}

fn main() {
    // configure logger and set log level
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Debug)
        .init();

    install_signal_handlers();

    let cli = CLI::parse();
    match cli.subcmd {
        CLISubCmd::Hv { subcmd } => match subcmd {
            HvSubCmd::Enable => todo!(),
            HvSubCmd::Disable => todo!(),
        },
        CLISubCmd::Instance { subcmd } => match subcmd {
            InstanceSubCmd::List => todo!(),
            InstanceSubCmd::Init => instance::init_shim(),
            InstanceSubCmd::Execute(args) => instance::execute(args),
            InstanceSubCmd::Remove { instance_id } => instance::remove_instance(instance_id as _),
        },
        CLISubCmd::Microvm { subcmd } => match subcmd {
            MicroVMSubCmd::Create(args) => {
                microvm::create_microvm(args).expect("Failed to create microvm")
            }
        },
        CLISubCmd::Loader(args) => loader::local_execute(args),
    }
}
