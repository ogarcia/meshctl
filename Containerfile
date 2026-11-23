# syntax=docker/dockerfile:1.28
#
# meshctl in a container: the daemon, with its interface embedded, on Alpine.
# The image is tagged after what it was built from: "master" for the latest
# on master, and a version once there are releases.
#
# Local build (compiles from source):
#   podman build -t meshctl:dev .
#
# CI build (binaries pre-compiled and placed in dist/<arch>/, arch = amd64|arm64
# to match Docker's automatic TARGETARCH; buildx picks the right subdir per
# platform, so the same context yields a multi-arch manifest):
#   podman build --build-arg BUILDER=prebuilt .
#
# Running it: the panel and the API on port 7434, what it keeps in a volume.
#   podman run -d --name meshctl -p 7434:7434 \
#       -v meshctl:/var/lib/meshctl ghcr.io/ogarcia/meshctl:master
#
# A radio on USB is a device passed in, which the container's user must be
# allowed to open: with rootless podman, keep the host's groups (the one that
# owns the device: dialout, or uucp on some distributions); otherwise add that
# group by its number.
#   podman run … --device /dev/ttyACM0 --group-add keep-groups …
#   docker run … --device /dev/ttyACM0 --group-add "$(stat -c %g /dev/ttyACM0)" …
#
# Environment variables (runtime):
#   MESHCTL_LISTEN   Where to serve the panel and the API (default here:
#                    0.0.0.0:7434, every interface of the container)
#   MESHCTL_DATA     Where to keep the database (default here: /var/lib/meshctl)
#   MESHCTL_TOKEN    An access token for the panel and the API. Set it unless
#                    something else in front already asks who is there
#   RUST_LOG         Log filter (default: info)

# ── Stage 1a: compile from source (default local path) ──────────────────────

# ARG declared before the first FROM so it is available as a global build
# argument. Default is 'builder' (local compile); CI passes
# --build-arg BUILDER=prebuilt to skip compilation and use dist/.
ARG BUILDER=builder

# Base-image and build-tool versions. Defaults mirror versions.env (the single
# source of truth); CI overrides them with --build-arg from that file. Declared
# before the first FROM so they apply to every FROM below. Override locally with
# e.g. --build-arg TRUNK_VERSION=x.y.z.
ARG ALPINE_VERSION=3.24
ARG RUST_VERSION=1
ARG TRUNK_VERSION=0.22.1

FROM docker.io/rust:${RUST_VERSION}-alpine${ALPINE_VERSION} AS builder

ARG TRUNK_VERSION

# musl-dev: C headers for SQLite, which libsqlite3-sys compiles in.
# wget: to download the trunk binary.
RUN apk add --no-cache musl-dev wget && \
    rustup target add wasm32-unknown-unknown && \
    wget -qO- "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-$(uname -m)-unknown-linux-musl.tar.gz" \
        | tar xz -C /usr/local/bin/

WORKDIR /app
COPY . .

# The interface first: web/dist, which the daemon embeds as it is compiled.
RUN cd web && trunk build --release

RUN cargo build --release --locked -p meshctl-server && \
    cp target/release/meshctl /usr/bin/meshctl

# ── Stage 1b: pre-built binaries injected from CI (dist/ in build context) ───

# TARGETARCH (amd64/arm64) is set automatically by buildx per target platform;
# it must be re-declared here to be usable in COPY. The dist/<arch>/ layout is
# produced by the container.yml build matrix.
FROM scratch AS prebuilt
ARG TARGETARCH
COPY --chmod=0755 dist/${TARGETARCH}/meshctl /usr/bin/meshctl

# ── Stage 1c: indirection — points to 'builder' or 'prebuilt' via build arg ──

FROM ${BUILDER} AS bins

# ── Stage 2: runtime ─────────────────────────────────────────────────────────

FROM docker.io/alpine:${ALPINE_VERSION}

# The daemon runs as its own user, in the dialout group for a radio on USB
# whose device is given that group.
RUN addgroup -S -g 10001 meshctl && \
    adduser  -S -G meshctl -u 10001 -h /var/lib/meshctl meshctl && \
    addgroup meshctl dialout && \
    mkdir -p /var/lib/meshctl && \
    chown -R meshctl:meshctl /var/lib/meshctl

COPY --from=bins /usr/bin/meshctl /usr/bin/meshctl

USER meshctl
WORKDIR /var/lib/meshctl

ENV MESHCTL_LISTEN=0.0.0.0:7434
ENV MESHCTL_DATA=/var/lib/meshctl

# The panel and the API: http://<host>:7434/
EXPOSE 7434/tcp

ENTRYPOINT ["/usr/bin/meshctl"]
