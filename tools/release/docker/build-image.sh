#!/usr/bin/env bash
# Build the minanode image from the release archives in DIST_DIR, as written
# by tools/release/build.sh or downloaded from a GitHub release: SHA256SUMS
# and one or both of mina-VERSION-linux-x86_64.tar.gz and
# mina-VERSION-linux-arm64.tar.gz. With SHA256SUMS.sig there too, the
# checksum list is first verified against docs/release/allowed_signers, or
# the file given with --signers (for an older release, the one from its tag:
# docs/release/DOCKER.md).
#
# A local build is for this machine's architecture and loads the image into
# the local image store. --push builds linux/amd64 and linux/arm64 with
# docker buildx and pushes them, with provenance and an SBOM; it requires a
# valid SHA256SUMS.sig.
#
# Runs on Linux and macOS (Bash 3.2).
set -euo pipefail

usage() {
  cat >&2 <<'EOF'
usage: build-image.sh VERSION DIST_DIR [options]

  --push REF         push to REF, e.g. ghcr.io/minanode/mina-rust:VERSION
  --tag REF          another tag to push with it (repeatable)
  --image REF        the local image's name (default ghcr.io/minanode/mina-rust:VERSION)
  --platform LIST    platforms, e.g. linux/arm64 (default: this machine's;
                     linux/amd64,linux/arm64 with --push)
  --no-load          build but keep no image (a dry run of the build)
  --metadata-file F  write buildx's metadata, with the pushed digest, to F
  --signers FILE     the allowed_signers file to verify SHA256SUMS.sig with
                     (default docs/release/allowed_signers of this checkout)
  --tool docker|podman
                     default: docker buildx if present, else podman (podman
                     builds one platform locally and cannot push here)
EOF
  exit 2
}

[ $# -ge 2 ] || usage
version=$1 dist=$2
shift 2
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || usage
[ -d "$dist" ] || { echo "no such directory: $dist" >&2; exit 2; }

push='' image='' platform='' load=1 metadata='' tool='' signers_file=''
extra_tags=''
while [ $# -gt 0 ]; do
  case $1 in
    --push) [ $# -ge 2 ] || usage; push=$2; shift 2 ;;
    --tag) [ $# -ge 2 ] || usage; extra_tags="$extra_tags $2"; shift 2 ;;
    --image) [ $# -ge 2 ] || usage; image=$2; shift 2 ;;
    --platform) [ $# -ge 2 ] || usage; platform=$2; shift 2 ;;
    --no-load) load=; shift ;;
    --metadata-file) [ $# -ge 2 ] || usage; metadata=$2; shift 2 ;;
    --signers) [ $# -ge 2 ] || usage; signers_file=$2; shift 2 ;;
    --tool) [ $# -ge 2 ] || usage; tool=$2; shift 2 ;;
    *) usage ;;
  esac
done
[ -z "$extra_tags" ] || [ -n "$push" ] || { echo '--tag needs --push' >&2; exit 2; }

repo=$(cd "$(dirname "$0")/../../.." && pwd)
dockerfile=$repo/tools/release/docker/Dockerfile
signers=$repo/docs/release/allowed_signers
if [ -n "$signers_file" ]; then
  [ -f "$signers_file" ] || { echo "no such file: $signers_file" >&2; exit 2; }
  signers=$(cd "$(dirname "$signers_file")" && pwd)/$(basename "$signers_file")
fi
dist=$(cd "$dist" && pwd)

if [ -z "$tool" ]; then
  if command -v docker >/dev/null 2>&1 && docker buildx version >/dev/null 2>&1; then
    tool=docker
  elif command -v podman >/dev/null 2>&1; then
    tool=podman
  else
    echo 'needs docker with buildx, or podman' >&2
    exit 2
  fi
fi
case $tool in docker|podman) ;; *) usage ;; esac
if [ "$tool" = podman ] && [ -n "$push" ]; then
  echo 'pushing needs docker buildx (multi-arch, provenance and SBOM)' >&2
  exit 2
fi

native_platform() {
  case $(uname -m) in
    x86_64|amd64) echo linux/amd64 ;;
    arm64|aarch64) echo linux/arm64 ;;
    *) echo "no image for $(uname -m); give --platform" >&2; exit 2 ;;
  esac
}
if [ -z "$platform" ]; then
  if [ -n "$push" ]; then platform=linux/amd64,linux/arm64; else platform=$(native_platform); fi
fi
case $platform in
  *,*) [ "$tool" = docker ] || { echo 'podman builds one platform at a time' >&2; exit 2; } ;;
  *) ;;
esac

sha256_check() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum -c "$@"; else shasum -a 256 -c "$@"; fi
}

cd "$dist"
[ -f SHA256SUMS ] || { echo "$dist has no SHA256SUMS" >&2; exit 1; }
if [ -f SHA256SUMS.sig ]; then
  echo "Verifying SHA256SUMS.sig against $signers"
  ssh-keygen -Y verify -f "$signers" -I release@minanode.org -n file \
    -s SHA256SUMS.sig < SHA256SUMS
elif [ -n "$push" ]; then
  echo 'refusing to push: SHA256SUMS.sig is missing' >&2
  exit 1
else
  echo 'warning: no SHA256SUMS.sig; checking the archives against an unsigned list' >&2
fi

# Stage a build context with only what the Dockerfile copies.
context=$(mktemp -d "${TMPDIR:-/tmp}/mina-image.XXXXXX")
trap 'rm -rf "$context"' EXIT
cp SHA256SUMS "$context/"
revision=
for p in $(echo "$platform" | tr ',' ' '); do
  case $p in
    linux/amd64) arch=x86_64 ;;
    linux/arm64) arch=arm64 ;;
    *) echo "no release archive for $p" >&2; exit 2 ;;
  esac
  archive=mina-$version-linux-$arch.tar.gz
  [ -f "$archive" ] || { echo "$dist has no $archive" >&2; exit 1; }
  # The one line whose file name is exactly $archive, compared as a string.
  awk -v f="$archive" 'substr($0, 65) == "  " f' SHA256SUMS > "$context/check"
  if [ "$(wc -l < "$context/check" | tr -d ' ')" != 1 ] ||
      ! grep -Eq '^[0-9a-f]{64}  ' "$context/check"; then
    echo "SHA256SUMS has no single line for $archive" >&2
    exit 1
  fi
  sha256_check "$context/check"
  cp "$archive" "$context/"
  # The commit the archive was built from, for the image's revision label.
  commit=$(tar -xzOf "$archive" build-info.json |
    sed -n 's/^ *"commit": "\([0-9a-f]\{40\}\)",*$/\1/p')
  [ -n "$commit" ] || { echo "$archive: no commit in build-info.json" >&2; exit 1; }
  [ -z "$revision" ] || [ "$revision" = "$commit" ] ||
    { echo "the archives come from different commits" >&2; exit 1; }
  revision=$commit
done
rm -f "$context/check"

args="--build-arg VERSION=$version --build-arg REVISION=$revision --platform $platform"
if [ -n "$push" ]; then
  tags="--tag $push"
  for t in $extra_tags; do tags="$tags --tag $t"; done
  output="--push --provenance=mode=max --sbom=true"
else
  [ -n "$image" ] || image=ghcr.io/minanode/mina-rust:$version
  tags="--tag $image"
  if [ -n "$load" ]; then output=--load; else output=; tags=; fi
fi

echo "Building $platform from $dist (revision $revision) with $tool"
# The values above hold no spaces, so plain word splitting is safe here.
# shellcheck disable=SC2086
if [ "$tool" = docker ] && [ -n "$push" ]; then
  # The labels are on each arch's image; GHCR reads a multi-arch package's
  # source, description and licence from the index, so annotate it too.
  docker buildx build $args $tags $output \
    --annotation "index:org.opencontainers.image.source=https://github.com/minanode/mina-rust" \
    --annotation "index:org.opencontainers.image.description=Mina node in Rust, release $version for devnet" \
    --annotation "index:org.opencontainers.image.licenses=Apache-2.0" \
    --annotation "index:org.opencontainers.image.version=$version" \
    --annotation "index:org.opencontainers.image.revision=$revision" \
    ${metadata:+--metadata-file "$metadata"} -f "$dockerfile" "$context"
elif [ "$tool" = docker ]; then
  docker buildx build $args $tags $output -f "$dockerfile" "$context"
else
  [ -n "$load" ] || { echo 'podman always keeps the image; drop --no-load' >&2; exit 2; }
  podman build $args $tags -f "$dockerfile" "$context"
fi
if [ -n "$push" ]; then
  echo "Pushed $push$extra_tags"
elif [ -n "$load" ]; then
  echo "Built $image"
  echo "Try it: docker run --rm $image --version"
fi
