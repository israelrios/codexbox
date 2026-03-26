use std::ffi::OsString;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "codexbox", about = "Launch Codex inside a Podman sandbox")]
pub struct Cli {
    #[arg(
        long,
        env = "CODEXBOX_IMAGE",
        value_name = "IMAGE",
        help = "Podman image to run"
    )]
    pub image: Option<String>,

    #[arg(long, help = "Rebuild the sandbox image before launch")]
    pub rebuild_image: bool,

    #[arg(long, help = "Print the final podman run command and exit")]
    pub dry_run: bool,

    #[arg(
        short = 'p',
        long,
        value_name = "PORT",
        help = "Publish a port with podman syntax (repeatable)"
    )]
    pub publish: Vec<String>,

    #[arg(
        long,
        env = "CODEXBOX_CONTAINER_COMMAND",
        value_name = "COMMAND",
        conflicts_with = "codex_args",
        help = "Run a shell command inside the container instead of codex"
    )]
    pub container_command: Option<String>,

    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        help = "Extra arguments forwarded to codex"
    )]
    pub codex_args: Vec<OsString>,
}
