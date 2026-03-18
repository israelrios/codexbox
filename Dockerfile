FROM node:22-bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive \
    NPM_CONFIG_UPDATE_NOTIFIER=false \
    NPM_CONFIG_FUND=false

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        git \
        less \
        openssh-client \
        procps \
        ripgrep \
    && rm -rf /var/lib/apt/lists/*

RUN npm install --global @openai/codex@latest

ENTRYPOINT ["codex"]
