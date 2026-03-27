# codexbox

`codexbox` is a Rust CLI that launches Codex inside an ephemeral Podman sandbox while preserving a controlled set of host mounts, environment variables, certificates, and nested container tooling.

## What it does

- Starts `codex` inside a Podman container by default.
- Reuses selected host configuration such as `~/.codex`, `~/.gitconfig`, GitHub CLI config, and configured writable roots.
- Supports nested rootless Podman inside the sandbox.
- Can publish ports through the outer `podman run` command.
- Creates bind mounts for existing directories, files, and sockets referenced by forwarded environment variables so tools inside the sandbox can still reach them.
- Automatically exports images built or pulled inside codexbox and imports them back into the host Podman image store after the run.
- Can run a custom shell command inside the sandbox for validation and tests.
- Can load project-local port mappings and additional directories from `.codex/codexbox.json` in the starting directory.

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

Project-local defaults can be defined in `.codex/codexbox.json` in the directory where you start `codexbox`:

```json
{
  "publish": ["127.0.0.1:8080:80", "8443:443"],
  "add_dirs": ["../shared", "/tmp/cache"]
}
```

- `publish` entries are passed to Podman as `--publish` values.
- `add_dirs` entries are resolved relative to the starting directory when needed.
- Configured `add_dirs` are mounted automatically and appended to the Codex invocation as `--add-dir` entries.

Run a shell command inside the sandbox instead of `codex`:

```bash
cargo run -- --container-command 'podman info'
```

## Mounts and images

- `codexbox` forwards the filtered environment into the sandbox.
- If a forwarded environment variable points at an existing host directory, file, or Unix socket, `codexbox` creates a matching bind mount for that path.
- New env-var-derived mounts still go through the one-time approval flow stored in `~/.codexbox-conf.json`.
- Directories supplied with Codex `--add-dir` or from `.codex/codexbox.json` are mounted automatically and do not require approval.
- When Podman inside codexbox builds or downloads images, `codexbox` exports those images at the end of the run and loads them into the host Podman image store automatically.

## Test

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

## Project layout

- `src/cli.rs` - CLI options
- `src/launcher.rs` - launch orchestration
- `src/podman.rs` - Podman command planning and execution
- `src/mounts.rs` - mount planning
- `container-entrypoint.sh` - container startup logic
- `Containerfile` - sandbox image definition

See `spec.md` for the implementation specification.
