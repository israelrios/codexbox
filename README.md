# codexbox

`codexbox` starts an ephemeral rootless Podman container in the current shell directory and launches Codex automatically in yolo mode.

## Behavior

- Uses the local `podman` CLI to build and run the container.
- Installs `podman` and `podman-docker` in the image, so both `podman` and Docker-compatible `docker` commands are available inside the container.
- Installs `gh` and `glab` in the image for GitHub and GitLab CLI workflows.
- Installs Python 3, `pip`, `pytest`, and `basedpyright` in the image.
- Uses the current shell `PWD` as the container working directory.
- Maps the current directory into the container at the same absolute path.
- Maps `~/.codex` into the container at the same absolute path.
- Maps `~/.gitconfig` into the container read-only when it exists on the host.
- Maps `~/.config/gh` into the container read-only when it exists on the host.
- Maps `~/.config/glab-cli` into the container read-only when it exists on the host.
- Maps `~/.local/share/containers` and `~/.config/containers` into the container when those directories exist on the host.
- Maps each existing path listed in `sandbox_workspace_write.writable_roots` from `~/.codex/config.toml`.
- Forwards the invoking shell environment into the container, excluding keys matched by `vars-to-ignore.txt`.
- Forwards the invoking shell's current `PATH` and prepends it to the image `PATH`.
- Maps existing file, directory, and socket paths referenced by forwarded environment variables when they exist on the host, excluding paths under `/usr` and `/var`.
- Requires one-time interactive approval before adding new env-var-derived mounts, and stores approved paths in `~/.codexbox-conf.json`.
- Never adds an env-var-derived bind mount for the home directory root itself, though subpaths under home can still be approved.
- Reuses the host CA trust store by bind-mounting the host certificate paths read-only.
- Uses Podman host network mode.
- Requires rootless Podman on the host.
- Runs as `root` inside the container while bind-mounted files are still written as the invoking host user.
- Uses a generated nested Podman `storage.conf` that prefers `overlay` with `fuse-overlayfs` when `/dev/fuse` and the binary are available, and otherwise falls back to `vfs` to avoid overlay-on-overlay startup failures from host Podman storage settings.
- Starts `codex --dangerously-bypass-approvals-and-sandbox` automatically.

Missing writable roots are skipped so a stale path in `config.toml` does not break the launcher.

## Usage

Build and run automatically:

```bash
./codexbox
```

Forward extra Codex CLI arguments:

```bash
./codexbox -- --model gpt-5.4 --search
```

Print the generated Podman command:

```bash
./codexbox --print-command
```

Print the command without running anything:

```bash
./codexbox --dry-run
```

Force an image rebuild:

```bash
./codexbox --rebuild
```
