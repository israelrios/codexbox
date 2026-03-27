# codexbox

`codexbox` is a Rust CLI that launches Codex inside an ephemeral Podman sandbox while preserving a controlled set of host mounts, environment variables, certificates, and nested container tooling.

## What it does

- Starts `codex` inside a Podman container by default.
- Reuses selected host configuration such as `~/.codex`, `~/.gitconfig`, GitHub CLI config, and configured writable roots.
- Supports nested rootless Podman inside the sandbox.
- Can publish ports through the outer `podman run` command.
- Creates bind mounts for existing directories, files, and sockets referenced by forwarded environment variables so tools inside the sandbox can still reach them.
- Automatically exports images built or pulled inside codexbox and imports them back into the host Podman image store after the run.
- Can run a custom argv command inside the sandbox for validation and tests.
- Can load user-level default port mappings and additional directories from `~/.codexbox-conf.json`.
- Embeds its container assets and default ignore list into the binary, so the built executable does not need sidecar config files.

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

Force a rebuild of the embedded image before launch:

```bash
cargo run -- --rebuild-image
```

Publish one or more ports with Podman syntax:

```bash
cargo run -- -p 127.0.0.1:8080:80 -p 8443:443
```

User defaults can be defined in `~/.codexbox-conf.json`:

```json
{
  "approved_paths": ["/run/user/1000/podman/podman.sock"],
  "publish": ["127.0.0.1:8080:80", "8443:443"],
  "add_dirs": ["~/shared", "/tmp/cache"],
  "ignore_var_patterns": ["CUSTOM_*"],
  "directories": {
    "~/work/project": {
      "publish": ["3000:3000"],
      "add_dirs": ["~/project-extra"]
    }
  }
}
```

- `approved_paths` stores one-time approvals for env-derived read-only mounts.
- `publish` entries are passed to Podman as `--publish` values.
- `add_dirs` entries are resolved relative to your home directory when needed.
- `ignore_var_patterns` extends the built-in env-var ignore patterns with extra glob rules.
- `directories` applies extra `publish` and `add_dirs` entries when you launch `codexbox` from that directory or one of its descendants.
- Configured `add_dirs` are mounted automatically and appended to the Codex invocation as `--add-dir` entries.

Run an argv command inside the sandbox instead of `codex`:

```bash
cargo run -- --container-command podman info
```

## Mounts and images

- `codexbox` forwards the filtered environment into the sandbox.
- If a forwarded environment variable points at an existing host directory, file, or Unix socket, `codexbox` creates a matching bind mount for that path.
- Env vars whose values contain `://` are treated as URLs and never scanned for mount candidates.
- New env-var-derived mounts still go through the one-time approval flow stored in `~/.codexbox-conf.json`.
- Directories supplied with Codex `--add-dir` or from `~/.codexbox-conf.json` are mounted automatically and do not require approval.
- The launcher rebuilds the Podman image automatically when the embedded container assets change or when the image is missing.
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
