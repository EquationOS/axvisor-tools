mod vmm;

use clap::{Args, Parser, Subcommand};

#[macro_use]
extern crate log;

#[derive(Parser)]
#[command(name = "axcli")]
#[command(about = "CommandLine Interface for ArceOS Hypervisor", long_about = None)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
struct CLI {
    #[command(subcommand)]
    subcmd: CLISubCmd,
}

#[derive(Subcommand)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
enum CLISubCmd {
    /// Subcommands related to hypervisor itself.
    Hv {
        #[command(subcommand)]
        subcmd: HvSubCmd,
    },
    /// Subcommands related to the management of guest virtual machines.
    Vm {
        #[command(subcommand)]
        subcmd: VmSubCmd,
    },
}

#[derive(Subcommand)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
enum HvSubCmd {
    /// Enable arceos-hypervisor type1.5.
    Enable,
    /// Disable arceos-hypervisor type1.5.
    Disable,
}

#[derive(Subcommand)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
enum VmSubCmd {
    /// list the info of the vm
    List,
    Create(VmCreateArgs),
    Boot(VmBootShutdownArgs),
    Shutdown(VmBootShutdownArgs),
}

#[derive(Debug, Args)]
struct VmCreateArgs {
    #[arg(short, long)]
    pub config_path: String,
}

#[derive(Debug, Args)]
struct VmBootShutdownArgs {
    #[arg(short, long)]
    pub vmid: u64,
}

fn main() {
    // configure logger and set log level
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Debug)
        .init();

    let cli = CLI::parse();
    match cli.subcmd {
        CLISubCmd::Hv { subcmd } => match subcmd {
            HvSubCmd::Enable => todo!(),
            HvSubCmd::Disable => todo!(),
        },
        CLISubCmd::Vm { subcmd } => match subcmd {
            VmSubCmd::List => todo!(),
            VmSubCmd::Create(arg) => vmm::axvmm_create_vm(arg).expect("Failed in axvmm_create_vm"),
            VmSubCmd::Boot(arg) => {
                vmm::axvmm_boot_shutdown_vm(true, arg).expect("Failed to boot VM")
            }
            VmSubCmd::Shutdown(arg) => {
                vmm::axvmm_boot_shutdown_vm(false, arg).expect("Failed to shutdown VM")
            }
        },
    }
}
