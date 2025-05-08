mod elf;
mod hvc;
mod instance;

use clap::{Args, Parser, Subcommand};

#[macro_use]
extern crate log;

#[derive(Parser)]
#[command(name = "axcli")]
#[command(about = "CommandLine Interface for AxVisor", long_about = None)]
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
    /// Subcommands related to the management of the container instance.
    Instance {
        #[command(subcommand)]
        subcmd: InstanceSubCmd,
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
enum InstanceSubCmd {
    /// list the info of the instance
    List,
    /// Init instance runtime environment.
    Init,
    /// Create a new instance.
    Create(InstanceCreateArgs)
}

#[derive(Debug, Args)]
struct InstanceCreateArgs {
    #[arg(short, long)]
    pub elf_path: String,
    #[arg(short, long, default_value_t = false)]
    pub one2onemapping: bool,
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
        CLISubCmd::Instance { subcmd } => match subcmd {
            InstanceSubCmd::List => todo!(),
            InstanceSubCmd::Init => instance::init_shim(),
            InstanceSubCmd::Create(arg) => instance::create_instance(arg),

        },
    }
}
