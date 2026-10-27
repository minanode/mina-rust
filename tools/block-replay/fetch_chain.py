#!/usr/bin/env python3
"""Download a canonical chain of precomputed blocks for mina-block-replay.

Lists the public `mina_network_block_data` bucket from a starting height up to
the height of a given tip, reads each candidate block's parent hash from the
start of its JSON, walks back from the tip to the starting height, and
downloads that chain zstd-compressed (needs the `zstd` command).

    fetch_chain.py --from-height 548148 --tip-height 564044 \
        --tip 3NL9ixEeoyrPa8nLyyKsRnHNuMg6yoaztdFdK5ZbRmJkPqqBE5Hd --out blocks

Walking back by parent hash keeps blocks of other chains out, such as the
Berkeley blocks the bucket holds at the first Mesa heights under the same
names; `--genesis` checks where the chain is rooted.

Writes `<out>/chain.txt` (`<height> <state hash>`, oldest first) and
`<out>/<height>-<state hash>.json.zst`.
"""

import argparse
import concurrent.futures
import json
import os
import re
import subprocess
import sys
import urllib.parse
import urllib.request

BUCKET = "mina_network_block_data"
PARENT = re.compile(rb'"previous_state_hash"\s*:\s*"([^"]+)"')


def get(url, headers=None, attempts=4):
    for attempt in range(attempts):
        try:
            request = urllib.request.Request(url, headers=headers or {})
            return urllib.request.urlopen(request, timeout=300).read()
        except Exception:
            if attempt == attempts - 1:
                raise


def list_blocks(network, first, last):
    """(height, state hash) of every block in the bucket within the heights."""
    blocks = []
    for prefix in range(first // 1000, last // 1000 + 1):
        token = None
        while True:
            query = {
                "prefix": f"{network}-{prefix}",
                "fields": "items(name),nextPageToken",
                "maxResults": "1000",
            }
            if token:
                query["pageToken"] = token
            url = f"https://storage.googleapis.com/storage/v1/b/{BUCKET}/o?"
            page = json.loads(get(url + urllib.parse.urlencode(query)))
            for item in page.get("items", []):
                _, height, rest = item["name"].split("-", 2)
                if first <= int(height) <= last and rest.endswith(".json"):
                    blocks.append((int(height), rest[: -len(".json")]))
            token = page.get("nextPageToken")
            if not token:
                break
    return blocks


def block_url(network, height, state_hash):
    return f"https://storage.googleapis.com/{BUCKET}/{network}-{height}-{state_hash}.json"


def parent_of(network, block):
    head = get(block_url(network, *block), {"Range": "bytes=0-2047"})
    match = PARENT.search(head)
    if not match:
        raise RuntimeError(f"no parent hash in {block}")
    return block[1], match.group(1).decode()


def download(network, out, block):
    height, state_hash = block
    path = os.path.join(out, f"{height}-{state_hash}.json.zst")
    if os.path.exists(path):
        return
    data = get(block_url(network, height, state_hash))
    subprocess.run(["zstd", "-q", "-3", "-o", path + ".tmp"], input=data, check=True)
    os.replace(path + ".tmp", path)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--network", default="mainnet")
    parser.add_argument("--from-height", type=int, required=True)
    parser.add_argument("--tip-height", type=int, required=True)
    parser.add_argument("--tip", required=True, help="state hash of the tip")
    parser.add_argument(
        "--genesis",
        help="state hash the first block's parent must be: the network's genesis, "
        "which the bucket has no file for",
    )
    parser.add_argument("--out", required=True)
    parser.add_argument("--jobs", type=int, default=32)
    args = parser.parse_args()
    os.makedirs(args.out, exist_ok=True)

    candidates = list_blocks(args.network, args.from_height, args.tip_height)
    print(f"{len(candidates)} candidate blocks", file=sys.stderr)
    height_of = dict((h, height) for height, h in candidates)
    # A block's parent never changes, so parents read before are kept in
    # `parents.txt` (`<height> <state hash> <parent>`) and not read again.
    cache = os.path.join(args.out, "parents.txt")
    parents = {}
    if os.path.exists(cache):
        with open(cache) as f:
            parents = dict(line.split()[1:3] for line in f if line.strip())
    unread = [b for b in candidates if b[1] not in parents]
    print(f"reading the parents of {len(unread)} of them", file=sys.stderr)
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool, open(cache, "a") as f:
        for (height, h), (_, parent) in zip(unread, pool.map(lambda b: parent_of(args.network, b), unread)):
            parents[h] = parent
            f.write(f"{height} {h} {parent}\n")

    chain, current = [], args.tip
    while current in height_of and height_of[current] >= args.from_height:
        chain.append((height_of[current], current))
        current = parents[current]
    chain.reverse()
    if not chain or chain[0][0] != args.from_height:
        if chain:
            height, h = chain[0]
            others = sorted(b for b in candidates if b[0] == height - 1)
            sys.exit(
                f"the chain from {args.tip} does not reach {args.from_height}: "
                f"the parent {current} of {height} {h} is not in the bucket, "
                f"which has {len(others)} other blocks at {height - 1}"
            )
        sys.exit(f"the tip {args.tip} is not in the bucket at heights {args.from_height} to {args.tip_height}")
    if args.genesis and current != args.genesis:
        sys.exit(f"the chain from {args.tip} is rooted at {current}, not {args.genesis}")
    print(f"chain of {len(chain)} blocks, rooted at {current}", file=sys.stderr)
    with open(os.path.join(args.out, "chain.txt"), "w") as f:
        f.writelines(f"{height} {h}\n" for height, h in chain)

    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        list(pool.map(lambda b: download(args.network, args.out, b), chain))
    print("downloaded", file=sys.stderr)


if __name__ == "__main__":
    main()
