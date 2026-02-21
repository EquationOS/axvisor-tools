use clap::{Args, Subcommand};

#[derive(Subcommand, Debug)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
pub enum MicroVMSubCmd {
    Init,
    /// Create a new instance.
    Create(MicroVMCreateArgs),
}

#[derive(Debug, Args)]
pub struct MicroVMCreateArgs {
    /// Path to the configuration file in json format.
    #[arg(short, long)]
    pub config_file: String,
}
