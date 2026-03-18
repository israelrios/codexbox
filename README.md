# codex-docker

`codex-docker` starts an ephemeral Docker container in the current shell directory and launches Codex automatically in yolo mode.

## Behavior

- Uses the current shell `PWD` as the container working directory.
- Maps the current directory into the container at the same absolute path.
- Maps `~/.codex` into the container at the same absolute path.
- Maps each existing path listed in `sandbox_workspace_write.writable_roots` from `~/.codex/config.toml`.
- Loads the exported variables from `~/.xsessionrc` into the container environment.
- Maps directory paths referenced by those `~/.xsessionrc` variables when they exist on the host, excluding paths under `/usr` and `/var`.
- Reuses the host CA trust store by bind-mounting the host certificate paths read-only.
- Runs the container with the invoking user UID and GID.
- Starts `codex --dangerously-bypass-approvals-and-sandbox` automatically.

Missing writable roots are skipped with a warning so a stale path in `config.toml` does not break the launcher.

## Usage

Build and run automatically:

```bash
./codex-docker
```

Forward extra Codex CLI arguments:

```bash
./codex-docker -- --model gpt-5.4 --search
```

Print the generated Docker command:

```bash
./codex-docker --print-command
```

Print the command without running anything:

```bash
./codex-docker --dry-run
```

Force an image rebuild:

```bash
./codex-docker --rebuild
```
