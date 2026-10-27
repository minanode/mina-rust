#!/usr/bin/env bash
# Build a network's genesis configs from the RocksDB ledger tarballs that OCaml
# publishes for it, without an OCaml daemon:
#
#   tools/ledger-tool/genesis-from-tarballs.sh <OCaml config> <out dir> <name>
#
# <OCaml config> is the network's daemon config as OCaml ships it, which names
# its genesis, staking epoch and next epoch ledgers by hash, with each
# tarball's SHA3-256 (`s3_data_hash`), and has the genesis timestamp and the
# epoch seeds. For Mesa devnet, it is `genesis_ledgers/devnet.json` of OCaml
# 4.0.2 (`config_84b8d969.json` in the mina-devnet-config deb).
#
# In <out dir> it writes:
#   <name>.bin                    the prebuilt config, with the three ledgers
#                                 (for replays and tests: `--genesis` of
#                                 mina-block-replay, `--genesis-config`)
#   <name>-without-ledgers.json   what the genesis block commits to, without
#                                 the ledgers (`mina node --genesis-config`, a
#                                 web node's genesis URL)
#   tarballs.txt                  where each tarball comes from
#   download/                     the tarballs, kept to skip the download next
#                                 time, each checked against its SHA3-256
#   dump/                         the databases dumped by mina-rocksdb-scanner
#
# Each ledger's root is checked three times: the root stored in its database,
# the root ledger-tool computes from its accounts, and the config's hash.
#
# Needs curl, tar, openssl (for SHA3-256), OCaml 4.0.2's mina-rocksdb-scanner
# (run through scripts/ocaml-mina, so OCAML_MINA_PREFIX applies), and a built
# ledger-tool:
#   LEDGER_TOOL   ledger-tool binary (default: target/release/ledger-tool of
#                 this checkout)
set -euo pipefail

if [[ $# -ne 3 ]]; then
	sed -n '2,/^[^#]/s/^# \{0,1\}//p' "${BASH_SOURCE[0]}" >&2
	exit 2
fi
config=$(realpath "$1")
out=$2
name=$3
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
ledger_tool=${LEDGER_TOOL:-$repo/target/release/ledger-tool}
[[ -x $ledger_tool ]] || {
	echo "no ledger-tool at $ledger_tool: build it with" \
		"cargo build --release -p ledger-tool, or set LEDGER_TOOL" >&2
	exit 1
}

mkdir -p "$out/download" "$out/dump" "$out/tmp"
out=$(realpath "$out")

"$ledger_tool" --input "$config" --tarball-urls >"$out/tarballs.txt"
cat "$out/tarballs.txt"

while read -r ledger hash url sha3; do
	tarball=$out/download/$(basename "$url")
	if [[ ! -f $tarball ]]; then
		echo "downloading the $ledger ledger $hash"
		curl -fsSL -o "$tarball.part" "$url"
		mv "$tarball.part" "$tarball"
	fi
	if [[ $sha3 != - ]]; then
		got=$(openssl dgst -sha3-256 -r "$tarball" | cut -d' ' -f1)
		if [[ $got != "$sha3" ]]; then
			echo "$tarball: SHA3-256 $got, not the config's $sha3" >&2
			exit 1
		fi
		echo "$ledger: SHA3-256 matches the config's s3_data_hash"
	else
		echo "$ledger: the config has no s3_data_hash, so the tarball is unchecked" >&2
	fi

	# The scanner opens the database read-write, so it works on a fresh copy.
	db=$out/tmp/db-$ledger
	rm -rf "$db"
	mkdir -p "$db"
	tar -xzf "$tarball" -C "$db"
	TMPDIR=$out/tmp OCAML_MINA_BIN=mina-rocksdb-scanner "$repo/scripts/ocaml-mina" \
		dump --db-path "$db" --output-file "$out/dump/$ledger.hex"
	rm -rf "$db"
done <"$out/tarballs.txt"

"$ledger_tool" --input "$config" \
	--genesis-dump "$out/dump/genesis.hex" \
	--staking-dump "$out/dump/staking.hex" \
	--next-dump "$out/dump/next.hex" \
	--output "$out/$name.bin"
"$ledger_tool" --prebuilt "$out/$name.bin" --without-ledgers \
	--output "$out/$name-without-ledgers.json"
rmdir "$out/tmp" 2>/dev/null || true

ls -l "$out/$name.bin" "$out/$name-without-ledgers.json"
