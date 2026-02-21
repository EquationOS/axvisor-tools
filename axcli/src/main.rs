//! CommandLine Interface and host daemon process for Equation OS.

mod hvc;

#[allow(non_camel_case_types)]
#[allow(unused)]
mod ioctl;

#[cfg(feature = "microvm")]
mod microvm;

#[cfg(feature = "libos")]
mod libos;

#[allow(unused)]
mod utils;

use clap::{Parser, Subcommand};

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
    #[cfg(feature = "libos")]
    /// Subcommands related to the management of the container instance.
    Instance {
        #[command(subcommand)]
        subcmd: libos::InstanceSubCmd,
    },
    #[cfg(feature = "libos")]
    /// Subcommands related to the a local test loader.
    Loader(libos::ExecuteArgs),
    #[cfg(not(feature = "libos"))]
    /// Subcommands related to the management of the container instance.
    /// This subcommand need to be enabled with libos feature.
    Instance { _subcmd: String },
    #[cfg(not(feature = "libos"))]
    /// Subcommands related to the a local test loader.
    /// This subcommand need to be enabled with libos feature.
    Loader { _args: String },
    #[cfg(feature = "microvm")]
    /// Subcommands related to microVM management.
    Microvm {
        #[command(subcommand)]
        subcmd: microvm::MicroVMSubCmd,
    },
    #[cfg(not(feature = "microvm"))]
    /// Subcommands related to microVM management.
    /// This subcommand need to be enabled with microvm feature.
    Microvm { _subcmd: String },
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
        sa.sa_sigaction = sigbus_handler as *const () as usize;
        sa.sa_flags = SA_SIGINFO;
        sigaction(SIGBUS, &sa, ptr::null_mut());

        // Install SIGSEGV handler
        sa = std::mem::zeroed();
        sa.sa_sigaction = sigsegv_handler as *const () as usize;
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
        #[cfg(feature = "libos")]
        CLISubCmd::Instance { subcmd } => match subcmd {
            libos::InstanceSubCmd::List => todo!(),
            libos::InstanceSubCmd::Init => libos::instance::init_shim(),
            libos::InstanceSubCmd::Execute(args) => libos::instance::execute(args),
            libos::InstanceSubCmd::Remove { instance_id } => {
                libos::instance::remove_instance(instance_id as _)
            }
        },
        #[cfg(feature = "libos")]
        CLISubCmd::Loader(args) => libos::loader::local_execute(args),
        #[cfg(not(feature = "libos"))]
        CLISubCmd::Instance { _subcmd } => {
            unimplemented!("Instance management is not supported without libos feature")
        }
        #[cfg(not(feature = "libos"))]
        CLISubCmd::Loader { _args } => {
            unimplemented!("Local test loader is not supported without libos feature")
        }
        #[cfg(feature = "microvm")]
        CLISubCmd::Microvm { subcmd } => match subcmd {
            microvm::MicroVMSubCmd::Init => microvm::init_gate(),
            microvm::MicroVMSubCmd::Create(args) => {
                microvm::create_microvm(args).expect("Failed to create microvm")
            }
        },
        #[cfg(not(feature = "microvm"))]
        CLISubCmd::Microvm { _subcmd } => {
            unimplemented!("MicroVM management is not supported without microvm feature")
        }
    }
}
