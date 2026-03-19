FROM node:22-bookworm-slim

ARG GLAB_VERSION=1.89.0

ENV DEBIAN_FRONTEND=noninteractive \
    NPM_CONFIG_UPDATE_NOTIFIER=false \
    NPM_CONFIG_FUND=false

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        build-essential \
        bubblewrap \
        ca-certificates \
        curl \
        fuse-overlayfs \
        gh \
        git \
        jq \
        less \
        openssh-client \
        podman \
        podman-docker \
        python-is-python3 \
        python3 \
        python3-pip \
        python3-pytest \
        procps \
        ripgrep \
        slirp4netns \
        uidmap \
    && rm -rf /var/lib/apt/lists/*

RUN printf 'root:1:65536\n' >> /etc/subuid \
    && printf 'root:1:65536\n' >> /etc/subgid

RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
        amd64) glab_arch='amd64' ;; \
        arm64) glab_arch='arm64' ;; \
        armhf) glab_arch='armv6' ;; \
        ppc64el) glab_arch='ppc64le' ;; \
        s390x) glab_arch='s390x' ;; \
        *) echo "Unsupported architecture for glab: $arch" >&2; exit 1 ;; \
    esac; \
    tmpdir="$(mktemp -d)"; \
    curl -fsSL "https://gitlab.com/gitlab-org/cli/-/releases/v${GLAB_VERSION}/downloads/glab_${GLAB_VERSION}_linux_${glab_arch}.tar.gz" \
        | tar -xz -C "$tmpdir"; \
    install -m 0755 "$tmpdir/bin/glab" /usr/local/bin/glab; \
    rm -rf "$tmpdir"

RUN npm install --global @openai/codex@latest basedpyright

ENTRYPOINT ["codex"]
