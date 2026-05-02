ARG RUST_VERSION=1.95
ARG NODE_VERSION=22

FROM rust:${RUST_VERSION}-bookworm AS builder

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --locked --release -p vik-cli

FROM node:${NODE_VERSION}-bookworm-slim AS runtime

ARG CODEX_PACKAGE=@openai/codex@latest

ENV HOME=/home/vik \
    VIK_WORKFLOW_PATH=/workflow/WORKFLOW.md \
    CODEX_HOME=/home/vik/.codex \
    GH_CONFIG_DIR=/home/vik/.config/gh \
    GH_PROMPT_DISABLED=1 \
    GH_NO_UPDATE_NOTIFIER=1 \
    GH_NO_EXTENSION_UPDATE_NOTIFIER=1 \
    NPM_CONFIG_UPDATE_NOTIFIER=false

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        git \
        gnupg \
        jq \
        openssh-client \
    && mkdir -p /etc/apt/keyrings \
    && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
        | dd of=/etc/apt/keyrings/githubcli-archive-keyring.gpg \
    && chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
        > /etc/apt/sources.list.d/github-cli.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends gh \
    && npm install -g "${CODEX_PACKAGE}" \
    && npm cache clean --force \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/vik /usr/local/bin/vik
COPY docker/entrypoint.sh /usr/local/bin/vik-entrypoint

RUN useradd --create-home --shell /bin/bash vik \
    && mkdir -p /workflow /workspaces "${CODEX_HOME}" "${GH_CONFIG_DIR}" \
    && chown -R vik:vik /workflow /workspaces /home/vik \
    && chmod +x /usr/local/bin/vik-entrypoint

USER vik
WORKDIR /workflow
VOLUME ["/workflow"]

ENTRYPOINT ["/usr/local/bin/vik-entrypoint"]
CMD []
