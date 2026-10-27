#!/usr/bin/env python3
"""Group a replay chain's zkApp commands by shape, to see what a subset covers.

A command's shape is its call tree, with each account update's authorization
kind, whether its token is the default one, its balance change's sign, which
fields it updates (and how many app state fields), which account preconditions
it sets, and how many events and actions it emits. Two commands of one shape
exercise much the same transaction logic; values (keys, amounts, proofs) are
ignored.

Prints how many shapes there are, how many commands share each, and the height
each first appears at; with --to H, how many of them a replay to H covers. It
reads every block, but parses only those with zkApp commands. Needs the `zstd`
command for compressed blocks. See README.md.

    tools/regression/zkapp_shapes.py "$WORK/replay" --to 551260
"""

import argparse
import collections
import concurrent.futures
import json
import os
import subprocess

DEFAULT_TOKEN = "wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf"


def read(path):
    if os.path.exists(path + ".zst"):
        return subprocess.run(["zstd", "-dc", path + ".zst"], capture_output=True, check=True).stdout
    with open(path, "rb") as f:
        return f.read()


def update_shape(tree):
    body = tree["elt"]["account_update"]["body"]
    update = body["update"]
    return (
        body["authorization_kind"][0],
        body["token_id"] == DEFAULT_TOKEN,
        body["balance_change"]["sgn"][0],
        tuple(sorted(k for k, v in update.items() if k != "app_state" and v[0] != "Keep")),
        sum(1 for v in update["app_state"] if v[0] != "Keep"),
        tuple(sorted(k for k, v in body["preconditions"]["account"].items() if k != "state" and v[0] != "Ignore")),
        len(body["events"]),
        len(body["actions"]),
        tuple(update_shape(call) for call in tree["elt"].get("calls", [])),
    )


def zkapp_commands(blocks, height, state_hash):
    """(shape, fee payer) of each zkApp command in a block."""
    data = read(os.path.join(blocks, f"{height}-{state_hash}.json"))
    if b'"Zkapp_command"' not in data:
        return []
    # Memos and URIs may hold bytes that are not UTF-8.
    block = json.loads(data.decode("latin-1"))
    found = []
    for diff in block["data"]["staged_ledger_diff"]["diff"]:
        for command in (diff or {}).get("commands", []):
            kind, payload = command["data"]
            if kind == "Zkapp_command":
                shape = tuple(update_shape(u) for u in payload["account_updates"])
                found.append((shape, payload["fee_payer"]["body"]["public_key"]))
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("replay_dir", help="chain.txt, with the blocks in it or in blocks/")
    parser.add_argument("--to", type=int, action="append", default=[], help="report a replay to this height's coverage")
    parser.add_argument("--jobs", type=int, default=8)
    args = parser.parse_args()
    blocks = os.path.join(args.replay_dir, "blocks")
    if not os.path.isdir(blocks):
        blocks = args.replay_dir
    with open(os.path.join(args.replay_dir, "chain.txt")) as f:
        chain = [(int(h), s) for h, s in (line.split()[:2] for line in f if line.strip())]

    count, first_at, payers = collections.Counter(), {}, collections.defaultdict(set)
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        found = pool.map(lambda entry: zkapp_commands(blocks, *entry), chain)
        for (height, _), commands in zip(chain, found):
            for shape, payer in commands:
                count[shape] += 1
                first_at.setdefault(shape, height)
                payers[shape].add(payer)

    print(f"{sum(count.values())} zkApp commands in {len(chain)} blocks, {len(count)} shapes")
    for to in args.to:
        covered = [s for s, h in first_at.items() if h <= to]
        print(f"a replay to {to} has {len(covered)} shapes, shared by {sum(count[s] for s in covered)} commands")
    print("commands\tfirst at\tfee payers\taccount updates")
    for shape, n in count.most_common():
        updates = 0
        stack = list(shape)
        while stack:
            updates += 1
            stack.extend(stack.pop()[-1])
        print(f"{n}\t{first_at[shape]}\t{len(payers[shape])}\t{updates}")


if __name__ == "__main__":
    main()
