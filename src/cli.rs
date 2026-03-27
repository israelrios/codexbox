use clap::Parser;

use crate::config::PublishSpec;

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
        value_name = "PUBLISH",
        help = "Publish a port as [HOST_IP:]HOST_PORT:CONTAINER_PORT[/udp] (repeatable)"
    )]
    pub publish: Vec<PublishSpec>,

    #[arg(
        long,
        value_name = "ARG",
        num_args = 1..,
        allow_hyphen_values = true,
        conflicts_with = "codex_args",
        help = "Run an argv command inside the container instead of codex"
    )]
    pub container_command: Option<Vec<String>>,

    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        help = "Extra arguments forwarded to codex"
    )]
    pub codex_args: Vec<String>,
}
