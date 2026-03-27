use std::process;

use clap::Parser;

use codexbox::{cli::Cli, launch};

fn main() {
    let cli = Cli::parse();

    match launch(cli) {
        Ok(code) => process::exit(code),
        Err(err) => {
            eprintln!("codexbox: {err}");
            process::exit(1);
        }
    }
}
