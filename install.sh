#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
install_root="${CODEXBOX_INSTALL_ROOT:-$HOME/.local}"

cd "$repo_root"

cargo install --path . --locked --force --root "$install_root"

if [[ ":${PATH:-}:" != *":$install_root/bin:"* ]]; then
    printf 'installed codexbox to %s/bin\n' "$install_root"
    printf 'add %s/bin to PATH if needed\n' "$install_root"
fi
