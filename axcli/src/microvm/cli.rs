use clap::{Args, Subcommand};

#[derive(Subcommand, Debug)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
pub enum MicroVMSubCmd {
    Init,
    /// Create a new instance.
    Create(MicroVMCreateArgs),
    /// Stop a running instance and park it back to the EqVisor gate.
    Stop(MicroVMStopArgs),
    /// Remove a stopped non-VFIO instance from EqVisor host state.
    Remove(MicroVMRemoveArgs),
}

#[derive(Debug, Args)]
pub struct MicroVMCreateArgs {
    /// Path to the configuration file in json format.
    #[arg(short, long)]
    pub config_file: String,
}

#[derive(Debug, Args)]
pub struct MicroVMRemoveArgs {
    /// MicroVM instance ID to remove.
    #[arg(short, long)]
    pub instance_id: u64,
}

#[derive(Debug, Args)]
pub struct MicroVMStopArgs {
    /// MicroVM instance ID to stop.
    #[arg(short, long)]
    pub instance_id: u64,
}
