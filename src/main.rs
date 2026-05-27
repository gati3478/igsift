use anyhow::Result;
use clap::Parser;

use ig_mgr::cli::{Cli, Command};

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Init { force }) => {
            ig_mgr::init_tracing(0);
            ig_mgr::init(force)
        }
        Some(Command::Check {
            export_dir,
            rebuild_cache,
        }) => {
            ig_mgr::init_tracing(0);
            ig_mgr::check(&export_dir, rebuild_cache)
        }
        Some(Command::Run(args)) => {
            ig_mgr::init_tracing(args.verbose);
            ig_mgr::run(args)
        }
        None => {
            ig_mgr::init_tracing(cli.run_args.verbose);
            ig_mgr::run(cli.run_args)
        }
    }
}
