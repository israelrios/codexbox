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

if [ "${HOME:-}" != "" ] && [ "$HOME" != "/root" ]; then
    home_config_dir="$HOME/.config"
    home_containers_dir="$home_config_dir/containers"
    mkdir -p "$home_config_dir"
    if [ -L "$home_containers_dir" ] || [ ! -e "$home_containers_dir" ]; then
        ln -snf /root/.config/containers "$home_containers_dir"
    fi
fi

before_images_file=
after_images_file=
diff_images_file=

if [ "${CODEXBOX_IMAGE_EXPORT_DIR:-}" != "" ]; then
    mkdir -p "$CODEXBOX_IMAGE_EXPORT_DIR"
    before_images_file=$(mktemp)
    after_images_file=$(mktemp)
    diff_images_file=$(mktemp)
    podman images --format '{{.Repository}}|{{.Tag}}|{{.ID}}' | sort -u > "$before_images_file"
fi

mkdir -p "$XDG_RUNTIME_DIR/podman"
chmod 700 "$XDG_RUNTIME_DIR"
mkdir -p /var/run
ln -snf "$XDG_RUNTIME_DIR/podman/podman.sock" /var/run/docker.sock

podman system service --time=0 "$DOCKER_HOST" &
service_pid=$!

i=0
while [ ! -S "$XDG_RUNTIME_DIR/podman/podman.sock" ] && [ "$i" -lt 50 ]; do
    i=$((i + 1))
    sleep 0.1
done

if [ "$#" -eq 0 ]; then
    set -- --dangerously-bypass-approvals-and-sandbox
fi

set +e
if [ "${CODEXBOX_CONTAINER_COMMAND:-}" != "" ]; then
    /bin/sh -lc "$CODEXBOX_CONTAINER_COMMAND"
else
    codex "$@"
fi
command_status=$?
set -e

if [ "$after_images_file" != "" ]; then
    podman images --format '{{.Repository}}|{{.Tag}}|{{.ID}}' | sort -u > "$after_images_file"
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
