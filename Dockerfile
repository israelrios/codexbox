FROM node:22-bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive \
    NPM_CONFIG_UPDATE_NOTIFIER=false \
    NPM_CONFIG_FUND=false

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        build-essential \
        ca-certificates \
        curl \
        docker.io \
        gh \
        git \
        jq \
        less \
        openssh-client \
        python-is-python3 \
        python3 \
        python3-pip \
        python3-pytest \
        procps \
        ripgrep \
    && rm -rf /var/lib/apt/lists/*

RUN npm install --global @openai/codex@latest basedpyright

ENTRYPOINT ["codex"]
