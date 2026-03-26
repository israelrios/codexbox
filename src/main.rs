mod approval;
mod certs;
mod cli;
mod codex_config;
mod config;
mod env_filter;
mod env_mounts;
mod errors;
mod launcher;
mod mounts;
mod podman;
mod policy;

use std::process;

use clap::Parser;

use crate::cli::Cli;

fn main() {
    let cli = Cli::parse();

    match launcher::launch(cli) {
        Ok(code) => process::exit(code),
        Err(err) => {
            eprintln!("codexbox: {err}");
            process::exit(1);
        }
    }
}
