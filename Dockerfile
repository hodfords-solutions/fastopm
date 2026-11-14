# syntax=docker/dockerfile:1
# Shell-form CMD on purpose (same as Verdaccio: expands $VERDACCIO_*); dumb-init forwards signals.
# check=skip=JSONArgsRecommended
#
# fastopm image: drop-in replacement for `verdaccio/verdaccio` as an npm proxy cache.
#
# Same contract as the Verdaccio image:
#   - port 4873, config at /verdaccio/conf/config.yaml, volume /verdaccio/storage
#   - env VERDACCIO_PORT / VERDACCIO_PROTOCOL / VERDACCIO_ADDRESS build the --listen address
#   - runs as UID 10001 (group 0), so existing Verdaccio volumes keep their permissions
#   - `uid_entrypoint` + dumb-init, arbitrary-UID friendly (OpenShift)
#
# Multi-arch without QEMU: the Rust build always runs on the builder's native platform
# ($BUILDPLATFORM) and cross-compiles to $TARGETPLATFORM with tonistiigi/xx (clang + lld).
#
#   docker buildx build --platform linux/amd64,linux/arm64 -t hodfords/fastopm:dev .
#
# Health: HEALTHCHECK calls GET /-/ping (expects 200 `{}`); by hand:
#   curl -fsS http://localhost:4873/-/ping

ARG RUST_VERSION=1.100
ARG DEBIAN_RELEASE=trixie
ARG XX_VERSION=1.10.0
ARG CARGO_CHEF_VERSION=0.1.78

FROM --platform=$BUILDPLATFORM tonistiigi/xx:${XX_VERSION} AS xx

# ---------------------------------------------------------------------------------------------
# Toolchain on the build platform: rust + clang/lld for cross-linking + cargo-chef.
FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-slim-${DEBIAN_RELEASE} AS chef
COPY --from=xx / /
RUN apt-get update \
 && apt-get install -y --no-install-recommends clang lld file pkg-config \
 && rm -rf /var/lib/apt/lists/*
ARG CARGO_CHEF_VERSION
RUN cargo install --locked cargo-chef --version ${CARGO_CHEF_VERSION} \
 && rm -rf "$CARGO_HOME/registry"
WORKDIR /src

# ---------------------------------------------------------------------------------------------
# Dependency recipe: only changes when Cargo.toml / Cargo.lock change.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ---------------------------------------------------------------------------------------------
FROM chef AS builder
ARG TARGETPLATFORM
# Target libc / crt objects for the cross linker (no-op packages when native).
RUN xx-apt-get update \
 && xx-apt-get install -y --no-install-recommends gcc libc6-dev \
 && rm -rf /var/lib/apt/lists/*
# 1) Build dependencies only: cached as a layer until the recipe changes.
COPY --from=planner /src/recipe.json recipe.json
RUN xx-cargo chef cook --release --locked -p fastopm --recipe-path recipe.json
# 2) Build the workspace sources (docker/ is not copied, so editing it does not rebuild).
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN xx-cargo build --release --locked -p fastopm \
 && cp "target/$(xx-cargo --print-target-triple)/release/fastopm" /usr/local/bin/fastopm \
 && xx-verify /usr/local/bin/fastopm

# ---------------------------------------------------------------------------------------------
FROM debian:${DEBIAN_RELEASE}-slim AS runtime

ARG VERSION=dev
ARG REVISION=unknown
LABEL org.opencontainers.image.title="fastopm" \
      org.opencontainers.image.description="npm proxy cache in Rust, drop-in replacement for Verdaccio as a read-only cache" \
      org.opencontainers.image.source="https://github.com/hodfords-solutions/fastopm" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}"

ENV FASTOPM_APPDIR=/opt/fastopm \
    VERDACCIO_USER_NAME=fastopm \
    VERDACCIO_USER_UID=10001 \
    VERDACCIO_PORT=4873 \
    VERDACCIO_PROTOCOL=http \
    VERDACCIO_ADDRESS=[::]
ENV HOME=$FASTOPM_APPDIR

# ca-certificates: TLS to the uplinks; dumb-init: PID 1 like the Verdaccio image.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates dumb-init \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /verdaccio/storage /verdaccio/plugins /verdaccio/conf "$FASTOPM_APPDIR" \
 && useradd --system --uid "$VERDACCIO_USER_UID" --gid 0 --home-dir "$FASTOPM_APPDIR" \
      --no-create-home --shell /usr/sbin/nologin "$VERDACCIO_USER_NAME"

COPY --from=builder /usr/local/bin/fastopm /usr/local/bin/fastopm
COPY --chmod=0755 docker/uid_entrypoint docker/healthcheck /usr/local/bin/
COPY docker/config.yaml /verdaccio/conf/config.yaml

RUN chown -R "$VERDACCIO_USER_UID:0" /verdaccio/storage /verdaccio/conf "$FASTOPM_APPDIR" \
 && chmod -R g=u /verdaccio/storage /verdaccio/conf "$FASTOPM_APPDIR" /etc/passwd

WORKDIR $FASTOPM_APPDIR
USER $VERDACCIO_USER_UID
EXPOSE $VERDACCIO_PORT
VOLUME /verdaccio/storage

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 CMD ["healthcheck"]

ENTRYPOINT ["uid_entrypoint"]
CMD fastopm --config /verdaccio/conf/config.yaml --listen $VERDACCIO_PROTOCOL://$VERDACCIO_ADDRESS:$VERDACCIO_PORT
