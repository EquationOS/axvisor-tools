//! LibOS support for axcli.

pub mod instance;
mod shared_pages;

/// Just for test and debug purpose.
/// A User-level executor to boot ELF.
pub mod loader;

#[allow(static_mut_refs)]
mod proxy;

use clap::{Parser, Subcommand};

#[derive(Subcommand, Debug)]
#[command(args_conflicts_with_subcommands = true)]
#[command(flatten_help = true)]
pub enum InstanceSubCmd {
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

#[derive(Parser, Debug)]
#[command(trailing_var_arg = true)]
pub struct ExecuteArgs {
    #[arg(required = true)]
    exec_args: Vec<String>,
}
