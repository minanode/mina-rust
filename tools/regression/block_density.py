#!/usr/bin/env python3
"""Count what each block of a replay chain carries, to choose replay subsets.

Prints one tab-separated line per block in the chain file, oldest first:
height, zkApp commands, account updates, signed commands, and the block's
size in bytes. The counts come from the JSON's tags rather than a parse, which
is fast (about 20 s for 16,000 blocks) and matched a parse where checked. Needs
the `zstd` command for compressed blocks. See README.md.

    tools/regression/block_density.py "$WORK/replay" > density.tsv
"""

import argparse
import concurrent.futures
import os
import subprocess


def read(path):
    if os.path.exists(path + ".zst"):
        return subprocess.run(["zstd", "-dc", path + ".zst"], capture_output=True, check=True).stdout
    with open(path, "rb") as f:
        return f.read()


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("replay_dir", help="chain.txt, with the blocks in it or in blocks/")
    parser.add_argument("--jobs", type=int, default=8)
    args = parser.parse_args()
    blocks = os.path.join(args.replay_dir, "blocks")
    if not os.path.isdir(blocks):
        blocks = args.replay_dir
    with open(os.path.join(args.replay_dir, "chain.txt")) as f:
        chain = [line.split()[:2] for line in f if line.strip()]

    def count(entry):
        height, state_hash = entry
        data = read(os.path.join(blocks, f"{height}-{state_hash}.json"))
        tags = (b'"Zkapp_command"', b'"account_update_digest"', b'"Signed_command"')
        return [height, *(data.count(tag) for tag in tags), len(data)]

    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        for row in pool.map(count, chain):
            print(*row, sep="\t")


if __name__ == "__main__":
    main()
