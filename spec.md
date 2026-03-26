# Codexbox — Rust implementation specification

## 1. Purpose

`codexbox` is a Rust CLI launcher that starts **Codex** inside a **Podman** sandbox with a strict allowlist-based filesystem view, while allowing controlled access to specific user configuration, cache, credential, certificate, and tool paths.

The launcher must:

* allow only explicitly approved host paths outside the working directory
* support rootless Podman inside the sandbox
* support Docker-compatible workflows through Podman
* preserve the invoking user’s ownership on writable bind mounts
* automatically launch:

```bash
codex --dangerously-bypass-approvals-and-sandbox
```

inside the sandbox

---

## 2. Program name

The binary shall be named:

```text
codexbox
```

Primary execution model:

```bash
codexbox
```

This shall launch `codex --dangerously-bypass-approvals-and-sandbox` inside the prepared sandbox.

Optional future subcommands may exist, but the default behavior is launching Codex.

---

## 3. Core required behaviors

`codexbox` shall implement all of the following:

1. maps `~/.codex`
2. maps `~/.gitconfig`
3. maps `~/.config/gh`
4. maps `~/.config/glab-cli`
5. maps each existing path listed in `sandbox_workspace_write.writable_roots` from `~/.codex/config.toml`
6. skips missing writable roots, so stale entries in `config.toml` do not break launch
7. forwards the invoking shell environment, excluding keys matched by `vars-to-ignore.txt`
8. forwards the invoking shell’s current `PATH`
9. examines forwarded environment variable values and, when they reference existing host file, directory, or socket paths, mounts those paths read-only
10. requires one-time interactive approval before adding any new env-var-derived mount
11. stores approved env-var-derived mount paths in `~/.codexbox-conf.json`
12. never auto-approves or mounts the user’s home directory root itself via env-var-derived mounts, though subpaths under home may be approved
13. reuses the host CA trust store by bind-mounting host certificate paths read-only
14. starts `codex --dangerously-bypass-approvals-and-sandbox` automatically

---

## 4. High-level architecture

The crate shall contain these main components:

* `cli.rs` — entrypoint/options
* `config.rs` — reads codexbox config and approval DB
* `codex_config.rs` — parses `~/.codex/config.toml`
* `env_filter.rs` — captures and filters environment variables
* `env_mounts.rs` — discovers candidate env-var-derived mounts
* `approval.rs` — interactive one-time approval workflow
* `mounts.rs` — constructs mount plan
* `certs.rs` — host CA trust-store discovery
* `podman.rs` — Podman plan builder and integration
* `launcher.rs` — launches Codex
* `errors.rs` — typed errors

---

## 5. Execution flow

Normative startup flow:

1. determine invoking user context
2. load `vars-to-ignore.txt`
3. read the invoking shell environment
4. filter environment variables
5. load `~/.codex/config.toml`
6. collect writable roots from `sandbox_workspace_write.writable_roots`
7. collect fixed required mounts
8. discover env-var-derived mount candidates
9. remove disallowed candidates
10. compare remaining candidates against `~/.codexbox-conf.json`
11. interactively prompt for approval for new candidates
12. persist newly approved candidates
13. discover CA trust-store paths
14. build Podman launch plan
15. launch:

```bash
codex --dangerously-bypass-approvals-and-sandbox
```

---

## 6. Fixed required mounts

The launcher shall always attempt to mount the following host paths when they exist.

### 6.1 Required writable mounts

These shall be mounted read-write:

* current working directory
* `~/.codex`
* each existing path listed in:

  * `sandbox_workspace_write.writable_roots` from `~/.codex/config.toml`

If an entry from `writable_roots` does not exist, it shall be skipped silently or logged at debug level, but must not fail the launch.

### 6.2 Required read-only mounts

These shall be mounted read-only when they exist:

* `~/.gitconfig`
* `~/.config/gh`
* `~/.config/glab-cli`
* host CA trust-store paths
* env-var-derived approved mounts

---

## 7. Codex config parsing

The launcher shall read:

```text
~/.codex/config.toml
```

and extract:

```toml
[sandbox_workspace_write]
writable_roots = [ ... ]
```

### 7.1 Rules for `writable_roots`

For each entry:

* expand `~`
* expand relative paths only if the codex config format explicitly permits them; otherwise treat them as invalid and skip with warning
* canonicalize where practical
* if the path exists on host, mount read-write
* if the path does not exist, skip it
* missing or stale roots must not abort startup

Suggested Rust model:

```rust
pub struct CodexToml {
    pub sandbox_workspace_write: Option<SandboxWorkspaceWrite>,
}

pub struct SandboxWorkspaceWrite {
    pub writable_roots: Option<Vec<PathBuf>>,
}
```

---

## 8. Environment forwarding

`codexbox` shall forward the invoking shell environment into the sandbox, subject to filtering.

### 8.1 Filtering source

The launcher shall read ignored environment variables glob patterns from:

```text
vars-to-ignore.txt
```

The file format shall be simple and line-oriented:

* blank lines ignored
* lines beginning with `#` ignored
* each remaining line is a pattern or exact key matcher
* first implementation may use exact key match only

### 8.2 Forwarding rules

For each environment variable in the invoking shell:

* if its key matches `vars-to-ignore.txt`, do not forward it
* otherwise forward it unchanged
* explicitly preserve the current invoking shell `PATH`

This means the sandbox inherits the current shell’s `PATH`, not a reconstructed one.

---

## 9. Env-var-derived mount discovery

After filtering the environment, the launcher shall inspect the values of forwarded variables and discover candidate bind mounts.

### 9.1 Candidate detection

A forwarded environment variable value may yield one or more candidate host paths if it contains a path-like reference.

Supported cases in the initial implementation:

* value is an absolute path to an existing file
* value is an absolute path to an existing directory
* value is an absolute path to an existing Unix socket
* split colon-separated values and inspect each segment

Examples:

* `SSH_AUTH_SOCK=/run/user/1000/keyring/ssh`
* `GPG_AGENT_INFO=/run/user/...`
* `MY_CERT=/home/user/certs/dev.pem`
* `SOME_DIR=/opt/tooling/cache`

### 9.2 Mount mode

All env-var-derived mounts shall be mounted **read-only**.

### 9.3 Exclusions

The launcher shall never add an env-var-derived bind mount for the user’s home directory root itself.

Example:

* `/home/israel` → forbidden
* `/home/israel/.ssh` → forbidden
* `/home/israel/projects/foo` → may be approved

This rule applies even if the home root appears in an environment variable and even if the user attempts to approve it interactively.

### 9.4 Existence requirement

Only existing host paths may become candidates.

If the referenced path does not exist on host, it shall not be added and no prompt shall be shown.

---

## 10. Interactive approval model

New env-var-derived mount candidates require one-time interactive approval.

### 10.1 Approval storage

Approvals shall be stored in:

```text
~/.codexbox-conf.json
```

Suggested schema:

```json
{
  "approved_paths": [
    "/run/user/1000/keyring/ssh",
    "/home/israel/.ssh",
    "/home/israel/.config/something"
  ]
}
```

Suggested Rust type:

```rust
pub struct CodexboxApprovalDb {
    pub approved_paths: BTreeSet<PathBuf>,
}
```

### 10.2 Approval prompt behavior

When a candidate path:

* exists
* is not home root
* is not already approved

the launcher shall ask interactively whether to allow it.

Example prompt shape:

```text
Allow readonly mount derived from environment variable SSH_AUTH_SOCK?
Host path: /run/user/1000/keyring/ssh
Approve and remember? [y/N]
```

### 10.3 One-time behavior

If approved:

* add to `~/.codexbox-conf.json`
* do not ask again on future launches

If denied:

* do not mount it
* do not persist approval

---

## 11. Host CA trust-store reuse

The launcher shall reuse the host CA trust store by bind-mounting host certificate paths read-only.

### 11.1 Discovery behavior

The launcher shall probe known certificate/trust-store locations and mount whichever exist.

Typical paths to check include:

* `/etc/ssl/certs`
* `/etc/pki/tls/certs`
* `/etc/ca-certificates`
* `/etc/ssl/cert.pem`
* `/etc/pki/ca-trust`
* distribution-specific trust-store files or directories

### 11.2 Mount mode

All CA trust paths shall be mounted read-only.

### 11.3 Goal

Network tooling inside the sandbox should trust the same host CA roots as the invoking system.

---

## 12. Podman and Docker compatibility

`codexbox` shall support rootless Podman inside the sandbox.

Suggested base podman command: 
```bash
podman run --rm -it --sysctl net.ipv4.ip_unprivileged_port_start=0 --device /dev/net/tun --device /dev/fuse
```

### 12.1 Persistent user Podman storage

Any Podman image created inside the sandbox must be stored in the user’s normal Podman storage.

## 13. Podman policy

The launcher shall construct a Podman environment that:

* mounts explicit allowlisted host paths only
* uses read-only or read-write bind mode per policy
* preserves the invoking user identity
* launches Codex directly as the inner command

Recommended inner launch target:

```bash
codex --dangerously-bypass-approvals-and-sandbox
```

### 13.1 Required mount classes

#### User/config mounts

* `~/.codex` read-write
* `~/.gitconfig` read-only
* `~/.config/gh` read-only
* `~/.config/glab-cli` read-only

#### Dynamic mounts

* writable roots from `~/.codex/config.toml`
* approved env-var-derived readonly mounts
* CA trust paths readonly

---

## 14. Default launch command

The launcher’s default behavior shall be equivalent to:

```bash
codex --dangerously-bypass-approvals-and-sandbox
```

No extra approval or sandbox flags shall be expected from the user.

Extra params should be forwarded to codex,

---

## 15. Rust crate structure

Recommended layout:

```text
codexbox/
  src/
    main.rs
    cli.rs
    config.rs
    codex_config.rs
    env_filter.rs
    env_mounts.rs
    approval.rs
    certs.rs
    policy.rs
    mounts.rs
    podman.rs
    launcher.rs
    errors.rs
```

---

## 16. Key Rust data structures

### 16.1 Launcher config

```rust
pub struct LauncherConfig {
    pub ignore_var_patterns: Vec<String>,
    pub approval_db_path: PathBuf,
}
```

### 16.2 Approval DB

```rust
pub struct ApprovalDb {
    pub approved_paths: BTreeSet<PathBuf>,
}
```

### 16.3 Mount spec

```rust
pub enum MountMode {
    ReadOnly,
    ReadWrite,
    Tmpfs,
}

pub struct MountSpec {
    pub host: PathBuf,
    pub guest: PathBuf,
    pub mode: MountMode,
    pub source: MountSource,
}

pub enum MountSource {
    Fixed,
    CodexWritableRoot,
    EnvDerived { var_name: String },
    CaTrust,
    Podman,
}
```

### 16.4 Forwarded environment

```rust
pub struct ForwardedEnv {
    pub vars: BTreeMap<OsString, OsString>,
}
```

### 16.5 Candidate env mount

```rust
pub struct EnvMountCandidate {
    pub var_name: String,
    pub host_path: PathBuf,
}
```

---

## 17. Normative module responsibilities

### `codex_config.rs`

* parse `~/.codex/config.toml`
* extract `sandbox_workspace_write.writable_roots`
* ignore missing roots safely

### `env_filter.rs`

* load `vars-to-ignore.txt`
* filter environment variables
* preserve current `PATH`

### `env_mounts.rs`

* detect path-like values in forwarded vars
* resolve existing files/directories/sockets
* reject home root
* propose readonly candidates

### `approval.rs`

* load/save `~/.codexbox-conf.json`
* prompt for new candidates
* remember approvals

### `certs.rs`

* discover host certificate/trust paths
* return readonly mount specs for existing paths

### `launcher.rs`

* assemble final mount plan
* construct podman invocation
* execute Codex automatically

---

## 18. Acceptance criteria

`codexbox` is acceptable only if all of the following are true.

1. launching `codexbox` starts `codex --dangerously-bypass-approvals-and-sandbox`
2. `~/.codex` is mounted read-write
3. `~/.gitconfig` is mounted read-only if present
4. `~/.config/gh` is mounted read-only if present
5. `~/.config/glab-cli` is mounted read-only if present
6. existing paths from `sandbox_workspace_write.writable_roots` are mounted read-write
7. missing writable roots do not break launch
8. environment variables matched by `vars-to-ignore.txt` are excluded
9. `PATH` from the invoking shell is forwarded
10. env-var-referenced existing file, directory, and socket paths are detected as readonly mount candidates
11. unapproved env-var-derived mounts trigger a one-time interactive approval prompt
12. approved env-var-derived mounts are persisted in `~/.codexbox-conf.json`
13. the home directory root itself is never added as an env-var-derived bind mount
14. host CA trust-store paths are mounted read-only
15. Podman images built inside the sandbox are visible in the user’s normal Podman storage outside the sandbox
16. files written on writable host mounts remain owned by the invoking user

---

## 19. Recommended MVP order

Best implementation order:

1. Podman launcher
2. fixed mounts
3. Codex auto-launch
4. parse `~/.codex/config.toml`
5. writable roots support with missing-path skipping
6. environment forwarding + ignore list
7. env-var-derived readonly candidate detection
8. approval DB and prompt
9. CA trust-store discovery
10. Podman persistence

---

## 20. Explicit behavioral summary

These are mandatory and non-negotiable in the implementation:

* **always launch Codex automatically**
* **always map `~/.codex`**
* **always try to map `~/.gitconfig`, `~/.config/gh`, `~/.config/glab-cli` when present**
* **read writable roots from `~/.codex/config.toml`**
* **skip missing writable roots**
* **forward shell env except ignored vars**
* **forward current `PATH`**
* **derive readonly mounts from forwarded env vars when they reference existing files/dirs/sockets**
* **prompt once for new env-derived mounts**
* **persist approvals in `~/.codexbox-conf.json`**
* **never mount the home root itself from env-derived discovery**
* **reuse host CA trust store readonly**
* **preserve user-owned writes**
* **persist Podman images to the user’s real rootless Podman storage**
