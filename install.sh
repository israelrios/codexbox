#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
install_root="${CODEXBOX_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}"

cd "$repo_root"

cargo install --path . --locked --force --root "$install_root"

installed_bin="$install_root/bin/codexbox"
resolved_bin="$(command -v codexbox || true)"

if [[ -n "$resolved_bin" && "$resolved_bin" != "$installed_bin" ]]; then
    printf 'warning: codexbox resolves to %s, not %s\n' "$resolved_bin" "$installed_bin" >&2
    printf 'adjust PATH or set CODEXBOX_INSTALL_ROOT to the active bin directory\n' >&2
elif [[ ":${PATH:-}:" != *":$install_root/bin:"* ]]; then
    printf 'installed codexbox to %s/bin\n' "$install_root"
    printf 'add %s/bin to PATH if needed\n' "$install_root"
fi
