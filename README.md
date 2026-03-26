# codexbox

`codexbox` is a Rust CLI that launches Codex inside an ephemeral Podman sandbox while preserving a controlled set of host mounts, environment variables, certificates, and nested container tooling.

## What it does

- Starts `codex` inside a Podman container by default.
- Reuses selected host configuration such as `~/.codex`, `~/.gitconfig`, GitHub CLI config, and configured writable roots.
- Supports nested rootless Podman inside the sandbox.
- Creates an in-container `$HOME/.config/containers` symlink to `/root/.config/containers` instead of mounting the host `~/.config/containers`.
- Can publish ports through the outer `podman run` command.
- Can run a custom shell command inside the sandbox for validation and tests.

## Requirements

- Rust toolchain
- Podman available on the host

## Build

```bash
cargo build
```

## Run

Launch Codex in the sandbox:

```bash
cargo run --
```

Print the generated Podman command without running it:

```bash
cargo run -- --dry-run
```

Rebuild the image before launch:

```bash
cargo run -- --rebuild-image
```

Publish one or more ports with Podman syntax:

```bash
cargo run -- -p 127.0.0.1:8080:80 -p 8443:443
```

Run a shell command inside the sandbox instead of `codex`:

```bash
cargo run -- --container-command 'podman info'
```

## Test

```bash
cargo test
```

## Project layout

- `src/cli.rs` - CLI options
- `src/launcher.rs` - launch orchestration
- `src/podman.rs` - Podman command planning and execution
- `src/mounts.rs` - mount planning
- `container-entrypoint.sh` - container startup logic
- `Containerfile` - sandbox image definition

See `spec.md` for the implementation specification.
