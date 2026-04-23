#!/bin/sh
set -eu

: "${XDG_RUNTIME_DIR:=/tmp/podman-run-$(id -u)}"
export XDG_RUNTIME_DIR
export DOCKER_HOST="unix://$XDG_RUNTIME_DIR/podman/podman.sock"
: "${TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE:=$DOCKER_HOST}"
export TESTCONTAINERS_DOCKER_SOCKET_OVERRIDE
: "${TESTCONTAINERS_RYUK_DISABLED:=true}"
export TESTCONTAINERS_RYUK_DISABLED

if [ "${CODEXBOX_PATH_PREFIX+x}" = x ]; then
    export PATH="${CODEXBOX_PATH_PREFIX}:$PATH"
fi

rewrite_root_home() {
    passwd_path=${CODEXBOX_PASSWD_PATH:-/etc/passwd}
    root_home=${HOME:-}

    [ "$root_home" != "" ] || return 0
    [ -f "$passwd_path" ] || return 0
    [ -w "$passwd_path" ] || return 0

    passwd_dir=$(dirname "$passwd_path")
    passwd_tmp=$(mktemp "$passwd_dir/passwd.codexbox.XXXXXX")
    cleanup_passwd_tmp() {
        rm -f "$passwd_tmp"
    }
    trap cleanup_passwd_tmp EXIT HUP INT TERM

    if ! awk -F: -v home="$root_home" '
        BEGIN {
            OFS = FS
            found_root = 0
        }
        $1 == "root" {
            $6 = home
            found_root = 1
        }
        { print }
        END {
            if (!found_root) {
                exit 1
            }
        }
    ' "$passwd_path" > "$passwd_tmp"; then
        echo "codexbox: failed to rewrite root home in $passwd_path" >&2
        exit 1
    fi

    cat "$passwd_tmp" > "$passwd_path"
    rm -f "$passwd_tmp"
    trap - EXIT HUP INT TERM
}

rewrite_root_home

merge_missing_tree() {
    src_dir=$1
    dest_dir=$2

    [ -d "$src_dir" ] || return 0
    mkdir -p "$dest_dir"

    (
        cd "$src_dir" || exit 1
        find . -mindepth 1 -print
    ) | while IFS= read -r rel_path; do
        src_path="$src_dir/$rel_path"
        dest_path="$dest_dir/$rel_path"

        if [ -d "$src_path" ]; then
            mkdir -p "$dest_path" 2>/dev/null || true
            continue
        fi

        if [ ! -e "$dest_path" ] && [ ! -L "$dest_path" ]; then
            mkdir -p "$(dirname "$dest_path")" 2>/dev/null || true
            cp -p "$src_path" "$dest_path" 2>/dev/null || cp "$src_path" "$dest_path" 2>/dev/null || true
        fi
    done
}

if [ "${CODEXBOX_SSH_KNOWN_HOSTS_SEED:-}" != "" ] && [ -f "$CODEXBOX_SSH_KNOWN_HOSTS_SEED" ]; then
    ssh_dir="${HOME:-/root}/.ssh"
    known_hosts_path="$ssh_dir/known_hosts"
    mkdir -p "$ssh_dir"
    chmod 700 "$ssh_dir" 2>/dev/null || true
    if [ ! -e "$known_hosts_path" ]; then
        cp "$CODEXBOX_SSH_KNOWN_HOSTS_SEED" "$known_hosts_path"
    fi
fi

if [ "${HOME:-}" != "" ] && [ "$HOME" != "/root" ]; then
    home_config_dir="$HOME/.config"
    home_containers_dir="$home_config_dir/containers"
    : "${CODEXBOX_GUEST_CONTAINERS_DIR:=/root/.config/containers}"
    mkdir -p "$home_config_dir"
    if [ -L "$home_containers_dir" ] || [ ! -e "$home_containers_dir" ]; then
        ln -snf "$CODEXBOX_GUEST_CONTAINERS_DIR" "$home_containers_dir"
    elif [ -d "$home_containers_dir" ]; then
        merge_missing_tree "$CODEXBOX_GUEST_CONTAINERS_DIR" "$home_containers_dir"
    fi

    for shell_init in .bashrc .bash_profile; do
        home_shell_init="$HOME/$shell_init"
        if [ ! -e "$home_shell_init" ] && [ ! -L "$home_shell_init" ]; then
            ln -s "/root/$shell_init" "$home_shell_init"
        fi
    done
fi

before_images_file=
after_images_file=
diff_images_file=

list_syncable_images() {
    # Ignore images exposed through the read-only additional image store.
    podman images --filter readonly=false --format '{{.Repository}}|{{.Tag}}|{{.ID}}' | sort -u
}

if [ "${CODEXBOX_IMAGE_EXPORT_DIR:-}" != "" ]; then
    mkdir -p "$CODEXBOX_IMAGE_EXPORT_DIR"
    before_images_file=$(mktemp)
    after_images_file=$(mktemp)
    diff_images_file=$(mktemp)
    list_syncable_images > "$before_images_file"
fi

mkdir -p "$XDG_RUNTIME_DIR/podman"
chmod 700 "$XDG_RUNTIME_DIR"
if mkdir -p /var/run 2>/dev/null; then
    ln -snf "$XDG_RUNTIME_DIR/podman/podman.sock" /var/run/docker.sock 2>/dev/null || true
fi

podman system service --time=0 "$DOCKER_HOST" &
service_pid=$!

service_wait_attempts="${CODEXBOX_PODMAN_SERVICE_WAIT_ATTEMPTS:-50}"
service_wait_delay="${CODEXBOX_PODMAN_SERVICE_WAIT_DELAY_SECS:-0.1}"
i=0
while [ ! -S "$XDG_RUNTIME_DIR/podman/podman.sock" ] && [ "$i" -lt "$service_wait_attempts" ]; do
    i=$((i + 1))
    sleep "$service_wait_delay"
done

if [ ! -S "$XDG_RUNTIME_DIR/podman/podman.sock" ]; then
    echo "codexbox: podman system service did not create $XDG_RUNTIME_DIR/podman/podman.sock" >&2
    kill "$service_pid" 2>/dev/null || true
    rm -f "$before_images_file" "$after_images_file" "$diff_images_file"
    exit 1
fi

if [ "$#" -eq 0 ]; then
    set -- codex --dangerously-bypass-approvals-and-sandbox
fi

set +e
"$@"
command_status=$?
set -e

if [ "$after_images_file" != "" ]; then
    list_syncable_images > "$after_images_file"
    comm -13 "$before_images_file" "$after_images_file" > "$diff_images_file"

    image_index=0
    while IFS='|' read -r repo tag image_id; do
        [ -n "$image_id" ] || continue

        image_ref=$image_id
        if [ "$repo" != "<none>" ] && [ "$tag" != "<none>" ]; then
            image_ref="$repo:$tag"
        fi

        archive_path="$CODEXBOX_IMAGE_EXPORT_DIR/image-$image_index.tar"
        podman save --format oci-archive --output "$archive_path" "$image_ref"
        image_index=$((image_index + 1))
    done < "$diff_images_file"

    rm -f "$before_images_file" "$after_images_file" "$diff_images_file"
fi

exit "$command_status"
