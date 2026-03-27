# codexbox

`codexbox` is a Rust CLI that launches Codex inside an ephemeral Podman sandbox while preserving a controlled set of
host mounts, environment variables, certificates, and nested container tooling.

## What it does

- Starts `codex` inside a Podman container by default.
- Reuses selected host configuration such as `~/.codex`, `~/.gitconfig`, GitHub CLI config, and configured writable
  roots.
- Supports nested rootless Podman inside the sandbox.
- Can publish ports through the outer `podman run` command.
- Creates bind mounts for existing directories, files, and sockets referenced by forwarded environment variables so
  tools inside the sandbox can still reach them.
- Automatically exports images built or pulled inside codexbox and imports them back into the host Podman image store
  after the run.
- Can run a custom argv command inside the sandbox for validation and tests.
- Can load user-level default port mappings, env-filter rules, and additional directories from `~/.codexbox-conf.json`.
- Embeds its container assets and default ignore list into the binary, so the built executable does not need sidecar
  config files.

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
  "publish": [
    { "host_ip": "127.0.0.1", "host_port": 8080, "container_port": 80 },
    { "host_port": 8443, "container_port": 443 }
  ],
  "add_dirs": ["~/shared", "/tmp/cache"],
  "block_var_patterns": ["CUSTOM_*"],
  "allow_var_patterns": ["SSH_AUTH_SOCK"],
  "directory_rules": [
    {
      "path": "~/work/project",
      "publish": [{ "host_port": 3000, "container_port": 3000 }],
      "add_dirs": ["~/project-extra"]
    }
  ]
}
```

- `approved_paths` stores one-time approvals for env-derived read-only mounts.
- `publish` entries are strict objects instead of free-form strings.
- `add_dirs` entries are resolved relative to your home directory when needed.
- `block_var_patterns` extends the built-in env-var block list with extra glob rules.
- `allow_var_patterns` re-allows vars that would otherwise be blocked by default or by `block_var_patterns`.
- `directory_rules` applies extra `publish` and `add_dirs` entries when you launch `codexbox` from that directory or one
  of its descendants.
- Configured `add_dirs` are mounted automatically and appended to the Codex invocation as `--add-dir` entries.

CLI `-p/--publish` accepts `CONTAINER_PORT`, `HOST_PORT:CONTAINER_PORT`, or `HOST_IP:HOST_PORT:CONTAINER_PORT`, with
optional `/udp`.

Run an argv command inside the sandbox instead of `codex`:

```bash
cargo run -- --container-command podman info
```

## Mounts and images

- `codexbox` forwards the filtered environment into the sandbox.
- If a forwarded environment variable points at an existing host directory, file, or Unix socket, `codexbox` creates a
  matching bind mount for that path.
- Env vars whose values contain `://` are treated as URLs and never scanned for mount candidates.
- New env-var-derived mounts still go through the one-time approval flow stored in `~/.codexbox-conf.json`.
- Directories supplied with Codex `--add-dir` or from `~/.codexbox-conf.json` are mounted automatically and do not
  require approval.
- The launcher rebuilds the Podman image automatically when the embedded container assets change, when the image is
  older than 7 days, or when the image is missing.
- Image rebuilds intentionally install the latest `@openai/codex` npm package.
- When Podman inside codexbox builds or downloads images, `codexbox` exports writable local images from the sandbox at
  the end of the run and loads them into the host Podman image store automatically. Images only visible through the
  host's read-only additional image store are ignored.

## Test

```bash
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
shellcheck container-entrypoint.sh
```

## Project layout

- `src/cli.rs` - CLI options
- `src/launcher.rs` - launch orchestration
- `src/config/` - Codexbox and Codex config parsing
- `src/podman/` - Podman image handling and command planning
- `src/sandbox/` - environment filtering, mount planning, and approval flow
- `src/user_context.rs` - user and working-directory detection
- `container-entrypoint.sh` - container startup logic
- `Containerfile` - sandbox image definition

See `spec.md` for the implementation specification.
