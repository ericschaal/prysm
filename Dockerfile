# syntax=docker/dockerfile:1

FROM node:24-bookworm-slim@sha256:c86ecead9e55a5210ac1727de38a4ca840bf03dcaf34dc9749addbf383b1920c AS ui-deps
WORKDIR /ui
COPY crates/web/prysm-web/control-ui/package.json crates/web/prysm-web/control-ui/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm \
    npm ci --no-audit --no-fund

FROM rust:1.99.1-bookworm@sha256:c9155318d07341301a78b45ecdd81af2e43b8b297d64e8fd4c5e80d7735607c3 AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends clang libclang-dev nasm pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY --from=ui-deps /usr/local/bin/node /usr/local/bin/node
COPY --from=ui-deps /usr/local/lib/node_modules/npm /usr/local/lib/node_modules/npm
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm
WORKDIR /src
COPY . .
COPY --from=ui-deps /ui/node_modules crates/web/prysm-web/control-ui/node_modules
ARG TARGETARCH
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=prysm-target-${TARGETARCH},target=/src/target,sharing=locked \
    cargo xtask codegen api --check \
    && cargo build --locked --release -p prysm --bin prysm \
    && install -D target/release/prysm /out/prysm \
    && strip /out/prysm

FROM debian:bookworm-slim@sha256:99c0b25c46d0214d5a2091092e7908907dcaecd6a962cde0ac700acee222b6f3 AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libgcc-s1 libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 prysm \
    && useradd --uid 10001 --gid prysm --no-create-home --shell /usr/sbin/nologin prysm \
    && install -d -o prysm -g prysm /config
COPY --from=build /out/prysm /usr/local/bin/prysm
COPY LICENSE /usr/share/doc/prysm/LICENSE
COPY apps/prysm-webos-capture/LICENSE.openh264 /usr/share/doc/prysm/LICENSE.openh264
LABEL org.opencontainers.image.title="Prysm" \
    org.opencontainers.image.description="TV and USB video capture for WLED, with an embedded web panel" \
    org.opencontainers.image.source="https://github.com/ericschaal/prysm" \
    org.opencontainers.image.licenses="Apache-2.0"
USER 10001:10001
WORKDIR /config
ENV PRYSM_CONFIG=/config/settings.json
EXPOSE 8787/tcp
ENTRYPOINT ["/usr/local/bin/prysm"]
