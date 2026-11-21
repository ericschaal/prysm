# syntax=docker/dockerfile:1
#
# prysm-headless. Build from the repository root; the context is the Rust and
# npm workspaces plus the licence (see Dockerfile.dockerignore):
#
#   docker build -f packaging/docker/Dockerfile -t prysm-headless:local .
#
# The other target, `artifacts`, holds the binary and the Debian package as
# /prysm-headless-linux-<arch> and /prysm-headless-linux-<arch>.deb, for
# `--output type=local,dest=DIR` (`cargo xtask dist linux-<arch> deb-<arch>`).
# See packaging/README.md.

# The bundle is plain JS/CSS, so it is built once on the build machine's own
# platform. Generated contracts are checked in and CI checks them and the
# types, so this stage only bundles and needs no Rust.
FROM --platform=$BUILDPLATFORM node:24-bookworm-slim@sha256:dc26ae92c9fb15cf6b1d63e20f0944b2efe0f115b20e00be48fb23fceb061c54 AS frontend
WORKDIR /src/frontend
COPY frontend/ ./
RUN --mount=type=cache,target=/root/.npm \
    npm ci --no-audit --no-fund \
    && npm run bundle

FROM rust:1.99.1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c AS build
# prysm-capture-device-sys -> v4l2-sys-mit generates its bindings with bindgen.
RUN apt-get update \
    && apt-get install -y --no-install-recommends clang libclang-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# VERSION: the product version, compiled in by prysm-version and read by
# build-deb.sh.
COPY Cargo.toml Cargo.lock VERSION ./
COPY crates/ crates/
COPY --from=frontend /src/frontend/dist/ frontend/dist/
ARG TARGETARCH
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=prysm-target-${TARGETARCH},target=/src/target,sharing=locked \
    cargo build --locked --release -p prysm-headless \
    && install -D target/release/prysm-headless /out/prysm-headless

FROM build AS deb
ARG TARGETARCH
# Here, not in `build`: a packaging change does not rebuild the binary.
COPY packaging/debian/ packaging/debian/
RUN packaging/debian/build-deb.sh /out/prysm-headless /deb/prysm-headless-linux-${TARGETARCH}.deb

# Named as in dist/ (crates/tools/xtask/src/dist.rs), with the architecture, so both
# architectures' files can sit in one directory.
FROM scratch AS artifacts
ARG TARGETARCH
COPY --from=build /out/prysm-headless /prysm-headless-linux-${TARGETARCH}
COPY --from=deb /deb/ /

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS runtime
# No FFmpeg: the "file" source needs FFmpeg 9+, which Debian does not ship;
# like the shipping image, this one offers the "device" and "remote" sources.
# User 10001 is in "video" (GID 44 here) for --device /dev/videoN; add the
# host's video GID with --group-add when it differs.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libgcc-s1 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 prysm \
    && useradd --uid 10001 --gid prysm --groups video --no-create-home \
        --home-dir /data --shell /usr/sbin/nologin prysm \
    && install -d -m 0700 -o prysm -g prysm /data
COPY --from=build /out/prysm-headless /usr/bin/prysm-headless
COPY LICENSE /usr/share/doc/prysm-headless/LICENSE
# The binary includes OpenH264 (BSD 2-clause), built from source.
COPY crates/adapters/h264/LICENSE.openh264 /usr/share/doc/prysm-headless/LICENSE.openh264
LABEL org.opencontainers.image.title="Prysm" \
    org.opencontainers.image.description="Video capture to WLED with a web portal (prysm-headless)" \
    org.opencontainers.image.source="https://github.com/ericschaal/prysm" \
    org.opencontainers.image.licenses="Apache-2.0"
# UID only: with an explicit GID Docker would drop the "video" membership.
USER 10001
WORKDIR /data
# A missing settings file means "unconfigured"; the portal creates it on the
# first save. /data must be writable by UID 10001.
VOLUME ["/data"]
# 8787: the portal, which also receives a slim TV's picture ("remote" source).
EXPOSE 8787/tcp
# The host stops on SIGTERM itself (session 10 s, portal 5 s); give it time
# with `docker stop -t 30` / compose `stop_grace_period`.
STOPSIGNAL SIGTERM
# Probes the portal's state endpoint without curl. Assumes the default
# --listen port and no --no-portal.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["bash", "-c", "exec 3<>/dev/tcp/127.0.0.1/8787 && printf 'GET /api/v1/state HTTP/1.1\\r\\nHost: localhost\\r\\nConnection: close\\r\\n\\r\\n' >&3 && head -n 1 <&3 | grep -q ' 200 '"]
# Arguments to `docker run IMAGE ...` are appended, such as --start or
# --allow-origin ORIGIN.
ENTRYPOINT ["/usr/bin/prysm-headless", "/data/settings.json"]
