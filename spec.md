# Codexbox specification

## 1. Scope

`codexbox` is a single self-contained Rust binary that launches Codex inside a Podman sandbox.

The binary must:

- expose only explicit host mounts
- preserve the invoking user on writable mounts
- support rootless Podman and Docker-compatible workflows through Podman
- embed its container build assets and default environment ignore list in the executable

The built artifact must not depend on extra bundled runtime files. User-managed files in the home directory are allowed.

## 2. Launch model

- The program name is `codexbox`.
- `codexbox [CODEx_ARGS...]` launches:

```bash
codex --dangerously-bypass-approvals-and-sandbox [CODEx_ARGS...]
```

- `--container-command <argv...>` replaces the default inner command with an argv vector.
- The inner command model is argv-based only. No shell-string command channel is supported.
- `--dry-run` prints the final `podman run` command and exits without mutating host state.
- `--rebuild-image-only` rebuilds the sandbox image and exits without starting the container.

`--dry-run` must not:

- create directories
- rebuild images
- write config
- prompt for approvals

## 3. Supported inputs

`codexbox` reads from:

1. the invoking process environment
2. `~/.codex/config.toml`
3. `~/.codexbox-conf.json`

No repo-local `codexbox` config file is supported.

Only UTF-8 is supported for:

- config files
- environment keys and values
- command arguments
- persisted path strings

Missing optional config files are treated as empty config.

## 4. User config

The user config file is:

```text
~/.codexbox-conf.json
```

Schema:

```json
{
  "approved_paths": ["/tmp/cache", "/etc/ssl/certs/custom.pem"],
  "approved_socket_vars": ["SSH_AUTH_SOCK"],
  "publish": [
    { "host_ip": "127.0.0.1", "host_port": 8080, "container_port": 80 }
  ],
  "add_dirs": ["~/shared"],
  "block_var_patterns": ["MY_SECRET_*"],
  "allow_var_patterns": ["MY_SECRET_PUBLIC_*"],
  "directory_rules": [
    {
      "path": "~/work/project",
      "publish": [{ "host_port": 3000, "container_port": 3000 }],
      "add_dirs": ["~/project-extra"]
    }
  ]
}
```

Rules:

- `approved_paths` stores globally approved read-only env-derived file and directory mounts
- `approved_socket_vars` stores globally approved env var names for Unix socket mounts
- `publish` adds validated port mappings
- `add_dirs` adds extra writable directory mounts and matching `codex --add-dir` arguments
- `block_var_patterns` extends the built-in environment block list
- `allow_var_patterns` re-allows variables blocked by default or by `block_var_patterns`
- `directory_rules` provides per-directory overrides for `publish` and `add_dirs` only

Path handling:

- `~` expands to the invoking user home directory
- top-level config paths that are not absolute are resolved from the home directory
- config parsing is strict: unknown fields are rejected
- CLI `--add-dir` relative paths are resolved from the current working directory
- configured directory rules apply when the current working directory is inside the configured directory
- if multiple directory overrides match, they are merged from shallowest match to deepest match

There is no per-directory `approved_paths`.

## 5. Codex config

`codexbox` reads:

```text
~/.codex/config.toml
```

and extracts:

```toml
[sandbox_workspace_write]
writable_roots = [ ... ]
```

Rules for `writable_roots`:

- expand `~`
- ignore missing or stale paths
- use existing paths as writable mounts
- do not fail startup because a configured root no longer exists

## 6. Mount policy

### 6.1 Writable mounts

The launcher must provide writable mounts for:

- the current working directory, except when it is exactly the user's home directory
- `~/.codex`
- existing `writable_roots` from `~/.codex/config.toml`
- valid directories from CLI or config `add_dirs`
- internal Podman persistence paths needed by `codexbox`

`add_dirs` are mounted only when they resolve to existing directories.
When the current working directory is the user's home directory, the launcher must emit a warning that explains the
home directory will not be bind-mounted and instructs the user to run from a project directory or explicitly configure
access with `--add-dir` or `writable_roots`.

### 6.2 Read-only mounts

The launcher must mount these read-only when present:

- `~/.gitconfig`
- `~/.config/gh`
- `~/.config/glab-cli`
- `/etc/docker`
- registry trust/auth sources needed by Docker- or Podman-configured hosts, such as `/etc/docker/certs.d`,
  `/etc/containers/certs.d`, `~/.docker/config.json`, and `~/.config/containers/auth.json`
- `~/.ssh/known_hosts` as a seed file for an ephemeral in-container copy
- approved env-derived paths
- discovered host CA trust paths

Writable files created through bind mounts must remain owned by the invoking user on the host.

## 7. Environment forwarding

The launcher forwards the invoking environment after filtering.

Rules:

- use an embedded built-in block-pattern list
- extend that list with `block_var_patterns` from `~/.codexbox-conf.json`
- allow explicit exceptions through `allow_var_patterns`
- do not forward any variable whose key matches the effective block list unless it also matches an allow pattern
- explicitly preserve the invoking `PATH`
- never forward internal `CODEXBOX_*` control variables
- forward `SSH_AUTH_SOCK` by default so an approved SSH agent socket can be mounted into the sandbox

## 8. Env-derived mount discovery

After filtering the environment, `codexbox` inspects forwarded values for mount candidates.

Candidate rules:

- skip any value containing `://`
- otherwise split values on `:`
- keep only absolute paths
- keep only existing files, directories, Unix sockets, or symlinks that resolve to them
- mount all accepted candidates read-only
- deduplicate repeated paths

Policy rules:

- the user home directory root itself must never be added through env-derived discovery
- subpaths under the home directory may be approved
- a candidate already covered by another mount should be ignored
- socket candidates are keyed by env var name so rotating socket paths can be re-mounted without re-approval

Approval rules:

- new env-derived candidates require interactive one-time approval
- approved file and directory paths are persisted to `approved_paths`
- approved socket env var names are persisted to `approved_socket_vars`
- denied paths are not persisted
- `--dry-run` uses only already-approved paths and must not prompt

## 9. Certificates

The launcher must reuse the host CA trust store by bind-mounting known host certificate locations read-only.

Typical probe targets include:

- `/etc/ssl/certs`
- `/etc/ssl/certs/ca-certificates.crt`
- `/etc/pki/tls/certs`
- `/etc/ca-certificates`
- `/etc/ssl/cert.pem`
- `/etc/pki/ca-trust`

Only existing host paths are mounted. When the host bundle path differs from the guest distro default,
the launcher may bind-mount the host CA bundle onto the guest's equivalent bundle path.

## 10. Podman behavior

The launcher builds and runs a Podman container for the sandbox.

Requirements:

- use rootless Podman
- keep the runtime image build context embedded in the binary
- rebuild the sandbox image when it is missing, when its embedded-asset fingerprint is stale, when the existing image is
  older than 7 days, or when `--rebuild-image` is set
- install the latest available `@openai/codex` npm package whenever the sandbox image is rebuilt
- avoid rebuilding when the current image fingerprint already matches
- keep Podman-created images available in the user environment outside the sandbox

The sandbox must support Docker-compatible workflows through Podman.

## 11. Effective startup flow

Normative flow:

If `--rebuild-image-only` is set, rebuild the selected image and exit before user-context detection or sandbox setup.

1. detect user context and current working directory
2. load `~/.codexbox-conf.json` and compute effective directory overrides
3. filter the invoking environment
4. load `~/.codex/config.toml` and collect existing writable roots
5. resolve `add_dirs`
6. build fixed mounts
7. discover env-derived mount candidates
8. prompt for approval and persist new approvals unless `--dry-run`
9. discover host CA trust paths
10. ensure the sandbox image is fresh unless `--dry-run`
11. execute Podman with the final argv-based inner command

## 12. Acceptance summary

The implementation is correct only if all of the following hold:

- `codexbox` launches Codex automatically by default
- the binary works without repo-local config files or bundled runtime sidecar files
- `~/.codexbox-conf.json` is the only persisted `codexbox` config file
- per-directory config affects only `publish` and `add_dirs`
- env-derived file and directory approvals are stored globally in `approved_paths`
- env-derived socket approvals are stored globally in `approved_socket_vars`
- `--dry-run` is side-effect free
- env-derived discovery ignores URL-like values containing `://`
- the sandbox image is rebuilt only when necessary, when older than 7 days, or explicitly requested
