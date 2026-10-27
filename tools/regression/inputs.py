#!/usr/bin/env python3
"""Check the harness inputs against their manifests, or write a manifest.

The inputs are the files the replays need that are not in the repo: the genesis
config, staged ledger dumps and precomputed blocks. A manifest in
tools/regression/inputs/ lists one set of them, a file per line:

    <sha256>  <size in bytes>  <path relative to the inputs directory>

with comment lines starting with `#`. The inputs directory is the main
checkout's .work/ on the development machine, and wherever the files were
downloaded to elsewhere, with the same layout. Only the standard library is
used. See tools/regression/README.md.

    tools/regression/inputs.py check tools/regression/inputs/*.txt
    tools/regression/inputs.py check --dir ~/mina-inputs --chain ~/mina-inputs/replay/chain.txt \\
        tools/regression/inputs/replay-from-dump.txt
    tools/regression/inputs.py chain tools/regression/inputs/replay-from-dump.txt > chain.txt
    tools/regression/inputs.py write --dir "$WORK" --title "..." \\
        --blocks replay/chain.txt 563044 564044 dumps/563044-....binprot > manifest.txt
"""

import argparse
import concurrent.futures
import hashlib
import json
from pathlib import Path
import tempfile
import os
import re
import subprocess
import sys

# A block's file name: `<height>-<state hash>.json`, maybe zstd-compressed.
BLOCK = re.compile(r"(?:^|/)(\d+)-(3N[1-9A-HJ-NP-Za-km-z]+)\.json(?:\.zst)?$")


def default_dir():
    """The main checkout's .work/, from this checkout or any of its worktrees."""
    here = os.path.dirname(os.path.abspath(__file__))
    try:
        common = subprocess.run(
            ["git", "-C", here, "rev-parse", "--path-format=absolute", "--git-common-dir"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return None
    return os.path.join(os.path.dirname(common), ".work")


def read_manifest(path):
    """[(sha256, size, relative path)] from a manifest file."""
    entries = []
    with open(path, encoding="utf-8") as lines:
        for number, line in enumerate(lines, 1):
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            fields = line.split(None, 2)
            if (
                len(fields) != 3
                or not re.fullmatch(r"[0-9a-f]{64}", fields[0])
                or not fields[1].isdigit()
            ):
                sys.exit(f"{path}:{number}: expected `<sha256> <size> <path>`, got {line!r}")
            entries.append((fields[0], int(fields[1]), fields[2]))
    return entries


def sha256(path):
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        while chunk := file.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def check_entry(directory, entry, sizes_only):
    """None if the file matches its entry, or what is wrong with it."""
    expected_hash, expected_size, relative = entry
    path = os.path.join(directory, relative)
    try:
        size = os.path.getsize(path)
    except OSError:
        return "missing"
    if size != expected_size:
        return f"{size} bytes, not {expected_size}"
    if not sizes_only and sha256(path) != expected_hash:
        return "sha256 differs"
    return None


def check_genesis_roots(directory, entries, ledger_tool):
    """Check regenerated ledgers, deliberately without a byte-identity claim."""
    # Node bootstrap pins; independently recorded from OCaml 4.0.2's packaged
    # config in tests/files/ocaml/mainnet-4.0.2-constants.json (P11d).
    canonical = Path(__file__).resolve().parents[2] / "genesis_ledgers/mesa-mainnet-without-ledgers.json"
    expected = json.loads(canonical.read_text())
    fields = ("genesis_ledger_hash", "staking_epoch_ledger_hash", "next_epoch_ledger_hash")
    failures = 0
    for _, _, relative in entries:
        path = Path(directory) / relative
        try:
            if relative.endswith(".bin"):
                if not ledger_tool:
                    raise ValueError("--ledger-tool is required to recompute binary roots")
                with tempfile.TemporaryDirectory(prefix="mina-genesis-check-") as tmp:
                    output = Path(tmp) / "recomputed-roots.json"
                    subprocess.run([ledger_tool, "--prebuilt", str(path), "--recompute-ledger-roots",
                                    "--output", str(output)], check=True, capture_output=True)
                    actual = json.loads(output.read_text())
            elif relative.endswith(".json"):
                actual = json.loads(path.read_text())
            else:
                raise ValueError("root checking is only supported for genesis JSON/binary")
            for field in fields:
                if actual.get(field) != expected[field]:
                    raise ValueError(f"{field} differs from the canonical mainnet genesis")
            checked = (
                "recomputed account roots" if relative.endswith(".bin")
                else "declared roots (no accounts in JSON)"
            )
            print(f"  {relative}: all three {checked} match (bytes not compared)")
        except (OSError, ValueError, subprocess.CalledProcessError) as error:
            print(f"  {relative}: FAILED: {error}")
            failures += 1
    return failures


def blocks_of(entries):
    """{height: state hash} of the blocks among the entries."""
    blocks = {}
    for _, _, relative in entries:
        if match := BLOCK.search(relative):
            blocks[int(match.group(1))] = match.group(2)
    return blocks


def read_chain(path):
    chain = {}
    with open(path, encoding="utf-8") as lines:
        for line in lines:
            fields = line.split()
            if len(fields) >= 2:
                chain[int(fields[0])] = fields[1]
    return chain


def check(args):
    directory = args.dir or default_dir()
    if not directory:
        sys.exit("not in a git checkout: pass --dir")
    failures = 0
    for manifest in args.manifests:
        entries = read_manifest(manifest)
        if args.genesis_roots:
            if args.sizes_only or args.chain:
                sys.exit("--genesis-roots cannot be combined with --sizes-only or --chain")
            if {entry[2] for entry in entries} != {
                "genesis-mainnet/mesa-mainnet.bin",
                "genesis-mainnet/mesa-mainnet-without-ledgers.json",
            }:
                sys.exit("--genesis-roots requires the mainnet genesis manifest only")
            failures += check_genesis_roots(directory, entries, args.ledger_tool)
            continue
        total = sum(size for _, size, _ in entries)
        with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
            results = list(
                pool.map(lambda entry: check_entry(directory, entry, args.sizes_only), entries)
            )
        bad = [(entry[2], problem) for entry, problem in zip(entries, results) if problem]
        for relative, problem in bad[:20]:
            print(f"  {relative}: {problem}")
        if len(bad) > 20:
            print(f"  ... and {len(bad) - 20} more")

        chain_problems = 0
        if args.chain:
            chain = read_chain(args.chain)
            for height, state_hash in sorted(blocks_of(entries).items()):
                if chain.get(height) != state_hash:
                    chain_problems += 1
                    if chain_problems <= 5:
                        print(f"  block {height} {state_hash}: not in {args.chain}")
            if chain_problems > 5:
                print(f"  ... and {chain_problems - 5} more blocks not in the chain file")
        checked = "sizes" if args.sizes_only else "sizes and sha256"
        status = "ok" if not bad and not chain_problems else "FAILED"
        print(
            f"{manifest}: {status}, {len(entries) - len(bad)} of {len(entries)} files"
            f" match ({checked}, {total / 1e6:.1f} MB in {directory})"
            + (f", {chain_problems} blocks not in the chain file" if chain_problems else "")
        )
        failures += len(bad) + chain_problems
    return 1 if failures else 0


def chain(args):
    """Print the chain file of the blocks the manifests list, oldest first.

    Blocks of several manifests can leave gaps, which replays don't cross: a
    replay from genesis stops at its `--to`, and one from a dump skips the
    lines up to its block.
    """
    blocks = {}
    for manifest in args.manifests:
        blocks.update(blocks_of(read_manifest(manifest)))
    heights = sorted(blocks)
    for before, after in zip(heights, heights[1:]):
        if after != before + 1:
            print(f"note: no blocks from {before + 1} to {after - 1}", file=sys.stderr)
    for height in heights:
        print(height, blocks[height])
    return 0


def write(args):
    directory = args.dir
    files = list(args.files)
    if args.blocks:
        chain_file, low, high = args.blocks
        chain_path = os.path.join(directory, chain_file)
        blocks_dir = os.path.join(os.path.dirname(chain_file), "blocks")
        for height, state_hash in sorted(read_chain(chain_path).items()):
            if int(low) <= height <= int(high):
                name = f"{height}-{state_hash}.json.zst"
                if not os.path.exists(os.path.join(directory, blocks_dir, name)):
                    name = name.removesuffix(".zst")
                files.append(os.path.join(blocks_dir, name))
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        hashes = list(pool.map(lambda f: sha256(os.path.join(directory, f)), files))
    for line in args.title:
        print(f"# {line}" if line else "#")
    print("#")
    print("# sha256, size in bytes, path relative to the inputs directory")
    for relative, digest in zip(files, hashes):
        size = os.path.getsize(os.path.join(directory, relative))
        print(f"{digest}  {size}  {relative}")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    commands = parser.add_subparsers(dest="command", required=True)

    check_parser = commands.add_parser("check", help="check files against manifests")
    check_parser.add_argument("manifests", nargs="+")
    check_parser.add_argument(
        "--dir", help="the inputs directory (default: the main checkout's .work/)"
    )
    check_parser.add_argument(
        "--chain", help="also check that every block listed is in this chain file"
    )
    check_parser.add_argument(
        "--sizes-only", action="store_true", help="compare sizes, not checksums"
    )
    check_parser.add_argument(
        "--genesis-roots", action="store_true",
        help="check regenerated mainnet genesis ledger roots instead of canonical bytes",
    )
    check_parser.add_argument("--ledger-tool", help="built ledger-tool for --genesis-roots")
    check_parser.add_argument("--jobs", type=int, default=8)
    check_parser.set_defaults(run=check)

    chain_parser = commands.add_parser(
        "chain", help="print the chain file of the blocks manifests list"
    )
    chain_parser.add_argument("manifests", nargs="+")
    chain_parser.set_defaults(run=chain)

    write_parser = commands.add_parser("write", help="print a manifest of files")
    write_parser.add_argument("files", nargs="*", help="paths relative to --dir")
    write_parser.add_argument("--dir", required=True, help="the inputs directory")
    write_parser.add_argument(
        "--blocks",
        nargs=3,
        metavar=("CHAIN", "FROM", "TO"),
        help="add the blocks from FROM to TO in the chain file CHAIN (relative to --dir),"
        " from the blocks/ directory next to it",
    )
    write_parser.add_argument(
        "--title", action="append", default=[], help="a comment line to start with"
    )
    write_parser.add_argument("--jobs", type=int, default=8)
    write_parser.set_defaults(run=write)

    args = parser.parse_args()
    return args.run(args)


if __name__ == "__main__":
    sys.exit(main())
