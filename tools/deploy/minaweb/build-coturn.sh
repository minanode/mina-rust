#!/usr/bin/env bash
# Build the relays' coturn from a pinned upstream release (MINAWEB.md,
# "coturn build"). Run it as an unprivileged user on a throwaway Ubuntu 24.04
# x86-64 machine, the relays' own release, with only these packages added:
#
#   sudo apt-get install --no-install-recommends build-essential cmake \
#       pkg-config libssl-dev libevent-dev libmicrohttpd-dev
#
#   build-coturn.sh OUT_DIR
#
# It downloads the release tarball, refuses it unless its SHA-256 matches the
# pin below, and builds turnserver without any database backend: the relays
# use only static-auth-secret. It writes OUT_DIR/coturn-4.18.0-noble-amd64.tar
# (bin/turnserver, BUILD-INFO) and prints the tar's SHA-256. Builds of one
# source on one package set give the same bytes; build twice on two machines
# and compare before installing. On a machine that has them, remove
# libsqlite3-dev and libhiredis-dev first.
set -euo pipefail
# Only the distribution's tools, whatever else the machine has installed.
export PATH=/usr/sbin:/usr/bin:/sbin:/bin

COTURN=4.18.0
# Tag 4.18.0, commit 23c6c1d32a3d2b21a56cee65d3203bcc4ea82d2b, committed
# 2026-09-07T22:45:21Z.
SHA256=28d55294ac596fbd129b293a85e7bb1c5dc4bd15b7fb55c500f355149e5f4e28
URL=https://github.com/coturn/coturn/archive/refs/tags/$COTURN.tar.gz
EPOCH=1788821121

die() {
	echo "build-coturn.sh: $*" >&2
	exit 1
}
[[ $# == 1 ]] || die "usage: build-coturn.sh OUT_DIR"
[[ $(id -u) != 0 ]] || die "run as an unprivileged user"
. /etc/os-release
[[ $ID == ubuntu && $VERSION_CODENAME == noble && $(uname -m) == x86_64 ]] ||
	die "build on Ubuntu 24.04 x86-64, the relays' release"
# CMake below turns off PostgreSQL, MySQL and MongoDB. SQLite and hiredis
# have no working switch: their headers must be absent.
for lib in sqlite3 hiredis; do
	! pkg-config --exists "$lib" || die "the $lib development package is installed: remove it"
done

out=$(mkdir -p "$1" && cd "$1" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl -fsSL --proto '=https' -o "$work/src.tar.gz" "$URL"
echo "$SHA256  $work/src.tar.gz" | sha256sum -c --quiet - || die "tarball does not match the pinned SHA-256"
tar -C "$work" -xzf "$work/src.tar.gz"
src=$work/coturn-$COTURN

export SOURCE_DATE_EPOCH=$EPOCH LC_ALL=C TZ=UTC
flags="-O2 -g0 -ffile-prefix-map=$work=/usr/src -fstack-protector-strong -D_FORTIFY_SOURCE=3 -fstack-clash-protection -fcf-protection"
cmake -S "$src" -B "$work/build" -DCMAKE_BUILD_TYPE=Release \
	-DCMAKE_C_FLAGS="$flags" -DCMAKE_CXX_FLAGS="$flags" \
	-DCMAKE_EXE_LINKER_FLAGS="-Wl,-z,relro,-z,now -Wl,--build-id=sha1" \
	-DCMAKE_POSITION_INDEPENDENT_CODE=ON \
	-DCMAKE_DISABLE_FIND_PACKAGE_PostgreSQL=ON \
	-DCMAKE_DISABLE_FIND_PACKAGE_mongo=ON -DWITH_MYSQL=OFF >"$work/cmake.log"
cmake --build "$work/build" --target turnserver -j "$(nproc)" >"$work/build.log"

bin=$work/build/bin/turnserver
strip --strip-unneeded "$bin"
"$bin" --version | grep -qx "$COTURN" || die "built turnserver does not report $COTURN"
# Only the libraries the relays need: OpenSSL, libevent and libmicrohttpd
# (Prometheus, off at run time). coturn's CMake build has no libsystemd, so
# coturn.service is Type=exec.
libs=$(ldd "$bin" | awk '{print $1}' | sort)
! grep -E 'sqlite|libpq|mysql|hiredis|mongo' <<<"$libs" || die "a database backend was linked"

stage=$work/stage
install -D -m 755 "$bin" "$stage/bin/turnserver"
{
	echo "coturn $COTURN"
	echo "source $URL"
	echo "source-sha256 $SHA256"
	echo "turnserver-sha256 $(sha256sum "$bin" | cut -d' ' -f1)"
	echo "built-on $PRETTY_NAME $(uname -m)"
	dpkg-query -W -f='package ${Package} ${Version}\n' gcc-13 cmake libssl-dev \
		libevent-dev libmicrohttpd-dev binutils libc6-dev
	echo "$libs" | sed 's/^/needs /'
} >"$stage/BUILD-INFO"
tar=$out/coturn-$COTURN-noble-amd64.tar
tar -C "$stage" --sort=name --owner=0 --group=0 --numeric-owner \
	--mtime="@$EPOCH" -cf "$tar" BUILD-INFO bin
sha256sum "$tar"
