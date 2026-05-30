use anyhow::Result;
use clap::Parser;

use igsift::cli::{Cli, Command};

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Init { force }) => {
            igsift::init_tracing(0);
            igsift::init(force)
        }
        Some(Command::Check {
            export_dir,
            rebuild_cache,
        }) => {
            igsift::init_tracing(0);
            igsift::check(&export_dir, rebuild_cache)
        }
        Some(Command::Run(args)) => {
            igsift::init_tracing(args.verbose);
            igsift::run(args)
        }
        None => {
            igsift::init_tracing(cli.run_args.verbose);
            igsift::run(cli.run_args)
        }
    }
}
