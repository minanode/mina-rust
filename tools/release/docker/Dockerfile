# The minanode image, built from the release archives rather than from source.
#
# The build context is a directory holding SHA256SUMS and the archives that
# tools/release/build.sh writes (mina-VERSION-linux-x86_64.tar.gz and
# mina-VERSION-linux-arm64.tar.gz); one of them is enough for a one-arch
# build. tools/release/docker/build-image.sh runs this with docker buildx or
# podman:
#
#   docker buildx build --platform linux/amd64,linux/arm64 \
#     --build-arg VERSION=0.20.0 -f tools/release/docker/Dockerfile dist/
#
# Needs BuildKit (Docker 23 or newer) or Buildah 1.33 or newer, for
# ADD --checksum.
#
# Every stage but the last runs on the build machine's own architecture
# ($BUILDPLATFORM), and the last has no RUN step, so a cross-arch build needs
# no emulation.
#
# Pinned inputs, and how to bump them:
# - Base images: pinned by the digest of their multi-arch index. To bump one,
#   take the new index digest from the registry, for example with
#   `docker buildx imagetools inspect gcr.io/distroless/base-debian13:nonroot`
#   (the "Digest:" line), or `crane digest` of the same reference.
# - tini: pinned by the SHA-256 of each static binary. Take a new release's
#   values from its tini-static-ARCH.sha256sum assets on
#   https://github.com/krallin/tini/releases, and the LICENSE file's from
#   the same tag.
# - jq, used only while building, to read build-info.json: pinned by the
#   SHA-256 of each static binary, from the sha256sum.txt asset of its
#   release on https://github.com/jqlang/jq/releases.

ARG BASE_IMAGE=gcr.io/distroless/base-debian13:nonroot@sha256:f9518b74815721f537a7458cc693b4406208c04fce23bbffab511516117f34fd
ARG UNPACK_IMAGE=docker.io/library/debian:trixie-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad
# Set by the builder; declared here for `FROM tini-...` and `FROM jq-...`.
ARG TARGETARCH
ARG BUILDARCH

# The node installs no SIGTERM or SIGINT handler. As PID 1 in a container it
# would ignore `docker stop`'s SIGTERM and be killed 10 seconds later. tini
# runs as PID 1 instead, passes SIGTERM on, and the node exits at once, as
# it does under systemd. It also reaps any child process that exits. `-e 143`
# reports that exit, by SIGTERM, as 0, as systemd counts it a clean stop.
FROM scratch AS tini-amd64
ADD --checksum=sha256:c5b0666b4cb676901f90dfcb37106783c5fe2077b04590973b885950611b30ee \
    https://github.com/krallin/tini/releases/download/v0.19.0/tini-static-amd64 /tini

FROM scratch AS tini-arm64
ADD --checksum=sha256:eae1d3aa50c48fb23b8cbdf4e369d0910dfc538566bfd09df89a774aa84a48b9 \
    https://github.com/krallin/tini/releases/download/v0.19.0/tini-static-arm64 /tini

FROM scratch AS tini-license
ADD --checksum=sha256:e5f46bca81266bdd511cf08018d66866870531794569c04f9b45f50dd23c28b0 \
    https://raw.githubusercontent.com/krallin/tini/v0.19.0/LICENSE /LICENSE

FROM tini-${TARGETARCH} AS tini

# jq for the unpack stage, which runs on the build machine's architecture:
# build on amd64 or arm64.
FROM scratch AS jq-amd64
ADD --checksum=sha256:b1c22172dd303f3be49e935aa56aa48a8b7a46e0bc838b4997d3bb451495870f \
    https://github.com/jqlang/jq/releases/download/jq-1.8.2/jq-linux-amd64 /jq

FROM scratch AS jq-arm64
ADD --checksum=sha256:8b85c817833814ddca00a144c33705546355afccf0cf39b188f3cdb48b852309 \
    https://github.com/jqlang/jq/releases/download/jq-1.8.2/jq-linux-arm64 /jq

FROM jq-${BUILDARCH} AS jq

# Check the archive for the target architecture against SHA256SUMS and lay out
# the image's files under /rootfs and /rootfs-data. The archive's checksum is
# the one SHA256SUMS line whose file name is exactly the archive's, compared
# as a string, not a pattern. build-info.json is parsed with jq: it must be
# a single JSON object whose top-level version and target are exactly the
# ones asked for.
FROM --platform=$BUILDPLATFORM ${UNPACK_IMAGE} AS unpack
ARG VERSION
ARG TARGETARCH
SHELL ["/bin/sh", "-eu", "-c"]
COPY SHA256SUMS mina-*-linux-*.tar.gz /in/
COPY --from=tini /tini /in/tini
COPY --from=tini-license /LICENSE /in/tini-LICENSE
COPY --from=jq /jq /in/jq
RUN case "${VERSION:-}" in ''|*[!0-9A-Za-z.-]*) false ;; esac \
      && printf '%s\n' "$VERSION" | grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?' \
      || { echo 'build with --build-arg VERSION=X.Y.Z' >&2; exit 1; }; \
    case "$TARGETARCH" in \
      amd64) arch=x86_64 ;; \
      arm64) arch=arm64 ;; \
      *) echo "no release archive for $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    archive="mina-${VERSION}-linux-${arch}.tar.gz"; \
    cd /in; \
    test -f "$archive" || { echo "$archive is not in the build context" >&2; exit 1; }; \
    awk -v f="$archive" 'substr($0, 65) == "  " f' SHA256SUMS > check; \
    test "$(wc -l < check)" -eq 1 && grep -Eq '^[0-9a-f]{64}  ' check \
      || { echo "SHA256SUMS has no single line for $archive" >&2; exit 1; }; \
    sha256sum -c --strict check; \
    mkdir -p /unpacked /rootfs/usr/local/bin /rootfs/usr/bin \
      /rootfs/usr/share/doc/mina /rootfs/usr/share/doc/tini /rootfs-data/data; \
    tar -xzf "$archive" -C /unpacked --no-same-owner; \
    chmod 0755 /in/jq; \
    /in/jq -se --arg v "$VERSION" --arg t "linux-$arch" \
      'length == 1 and (.[0] | type == "object" and .version == $v and .target == $t)' \
      /unpacked/build-info.json > /dev/null \
      || { echo "$archive is not version $VERSION for linux-$arch" >&2; exit 1; }; \
    install -m 0755 /unpacked/mina /rootfs/usr/local/bin/mina; \
    install -m 0755 /in/tini /rootfs/usr/bin/tini; \
    install -m 0644 /in/tini-LICENSE /rootfs/usr/share/doc/tini/LICENSE; \
    cd /unpacked; \
    cp -R LICENSE THIRD_PARTY_NOTICES THIRD_PARTY_LICENSES SOURCES README.txt \
      build-info.json /rootfs/usr/share/doc/mina/; \
    chmod -R u=rwX,go=rX /rootfs/usr/share/doc; \
    chmod 0700 /rootfs-data/data

FROM ${BASE_IMAGE}
ARG VERSION
ARG REVISION=
LABEL org.opencontainers.image.title="minanode" \
      org.opencontainers.image.description="Mina node in Rust, release ${VERSION} for devnet" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.source="https://github.com/minanode/mina-rust" \
      org.opencontainers.image.url="https://minanode.org/docs/docker/" \
      org.opencontainers.image.licenses="Apache-2.0"

COPY --from=unpack /rootfs/ /
# The volume's mount point, owned by the node's user. A new named volume
# copies this owner and mode (0700), so the node can write to it.
COPY --from=unpack --chown=65532:65532 /rootfs-data/ /

# Everything the node writes goes to /data: its work directory (MINA_HOME),
# and the verifier index and ledger caches it keeps under HOME.
# MINA_HTTP_ADDR=0.0.0.0 makes the dashboard reachable through a published
# port; publish it on 127.0.0.1 only (see the docs).
ENV MINA_HOME=/data \
    HOME=/data \
    MINA_HTTP_ADDR=0.0.0.0
USER 65532:65532
WORKDIR /data
VOLUME ["/data"]
EXPOSE 3000 8302

# Healthy once synced with a recent tip and at least one peer. It asks the
# node on port 3000 inside the container; change it if you change --port.
HEALTHCHECK --interval=1m --timeout=20s --start-period=10m --retries=3 \
    CMD ["/usr/local/bin/mina", "node", "status", "--check", "--status-only", \
         "--node", "http://127.0.0.1:3000"]

ENTRYPOINT ["/usr/bin/tini", "-e", "143", "--", "/usr/local/bin/mina"]
CMD ["node", "--network", "devnet"]
