use anyhow::Result;
use clap::Parser;

use ig_mgr::cli::Cli;

fn main() -> Result<()> {
    let cli = Cli::parse();
    ig_mgr::init_tracing(cli.verbose);
    ig_mgr::run(cli)
}
