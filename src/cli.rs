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

    #[arg(
        long,
        help = "Rebuild the sandbox image and exit",
        conflicts_with_all = ["dry_run", "publish", "container_command", "codex_args"]
    )]
    pub rebuild_image_only: bool,

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

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Cli;

    #[test]
    fn rebuild_image_only_conflicts_with_dry_run() {
        assert!(Cli::try_parse_from(["codexbox", "--rebuild-image-only", "--dry-run"]).is_err());
    }

    #[test]
    fn rebuild_image_only_accepts_custom_image() {
        let cli = Cli::parse_from([
            "codexbox",
            "--image",
            "localhost/codexbox:test",
            "--rebuild-image-only",
        ]);

        assert_eq!(cli.image.as_deref(), Some("localhost/codexbox:test"));
        assert!(cli.rebuild_image_only);
    }
}
