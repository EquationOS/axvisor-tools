mod hvc;
mod instance;
mod shared_pages;

/// Just for test and debug purpose.
/// A User-level executor to boot ELF.
mod loader;

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
    /// Create a new instance.
    #[command(subcommand)]
    Create(InstanceKind),
}

#[derive(Subcommand, Debug)]
enum InstanceKind {
    Static(InstanceCreateArgs),
    Load(ExecuteArgs),
}

#[derive(Debug, Args)]
struct InstanceCreateArgs {
    /// Path to the ELF file or binary file.
    #[arg(short, long)]
    pub file_path: String,
    /// Instance type, 0 for LibOS, 1 for kernel.
    #[arg(short, long, default_value_t = 0)]
    pub instance_type: usize,
    /// Use one2one mapping or coarse-grained mapping.
    #[arg(short, long, default_value_t = false)]
    pub one2onemapping: bool,
}

#[derive(Parser, Debug)]
#[command(trailing_var_arg = true)]
struct ExecuteArgs {
    #[arg(required = true)]
    exec_args: Vec<String>,
}

fn main() {
    // configure logger and set log level
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Trace)
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
            InstanceSubCmd::Create(kind) => match kind {
                InstanceKind::Static(arg) => instance::create_instance(arg),
                InstanceKind::Load(arg) => instance::load_junction(arg),
            },
        },
        CLISubCmd::Loader(args) => loader::local_create_junction(args),
    }
}
