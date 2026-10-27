#!/usr/bin/env python3
"""List the competing blocks around past epoch boundaries.

For each boundary in a chain.txt (found from the replay data), lists every
block the public precomputed bucket has at heights near the boundary, with
its epoch and slot. A height with blocks in both epochs means a node could
have switched between a tip in the new epoch and one in the old epoch.

    boundary_forks.py --blocks DIR --chain DIR/chain.txt [--around 3]
"""

import argparse
import os
import re
import sys
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import check_epoch_boundary as ceb  # noqa: E402

SLOT = re.compile(rb'"curr_global_slot_since_hard_fork":\{"slot_number":"(\d+)"')
PARENT = re.compile(rb'"previous_state_hash":"([^"]+)"')


def head(height, state_hash):
    req = urllib.request.Request(
        ceb.bucket_url(height, state_hash),
        headers={"Range": "bytes=0-16383", "user-agent": "mina-rust-epoch-check/1"},
    )
    data = urllib.request.urlopen(req, timeout=120).read()
    return int(SLOT.search(data).group(1)), PARENT.search(data).group(1).decode()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--blocks", required=True)
    ap.add_argument("--chain", required=True)
    ap.add_argument("--around", type=int, default=3)
    args = ap.parse_args()
    chain = []
    with open(args.chain) as f:
        for line in f:
            if line.strip():
                h, s = line.split()
                chain.append((int(h), s))
    canonical = dict(chain)
    # Find boundaries by bisection on the canonical chain's slots.
    slot_cache = {}

    def slot_at(i):
        if i not in slot_cache:
            h, s = chain[i]
            ps = ceb.read_protocol_state_zst(os.path.join(args.blocks, f"{h}-{s}.json.zst"))
            slot_cache[i] = int(ps["body"]["consensus_state"]["curr_global_slot_since_hard_fork"]["slot_number"])
        return slot_cache[i]

    last_epoch = slot_at(len(chain) - 1) // ceb.SLOTS_PER_EPOCH
    total_mixed = 0
    for epoch in range(1, last_epoch + 1):
        lo, hi = 0, len(chain) - 1
        while lo < hi:
            mid = (lo + hi) // 2
            if slot_at(mid) // ceb.SLOTS_PER_EPOCH >= epoch:
                hi = mid
            else:
                lo = mid + 1
        t_height = chain[lo][0]
        print(f"epoch {epoch}: T = {t_height} (canonical slot {slot_at(lo)})")
        for h in range(t_height - args.around, t_height + args.around + 1):
            rows = []
            for s in ceb.bucket_hashes_at(h):
                slot, parent = head(h, s)
                rows.append((slot, s, parent))
            epochs = {slot // ceb.SLOTS_PER_EPOCH for slot, _, _ in rows}
            mixed = len(epochs) > 1
            total_mixed += mixed
            print(f"  height {h}: {len(rows)} block(s)" + ("  <- both epochs" if mixed else ""))
            for slot, s, parent in sorted(rows):
                mark = "*" if canonical.get(h) == s else " "
                print(
                    f"    {mark} {s} epoch {slot // ceb.SLOTS_PER_EPOCH} slot {slot % ceb.SLOTS_PER_EPOCH}"
                    f" parent {ceb.short(parent)}"
                )
    print(f"\nheights with blocks in both epochs: {total_mixed}")


if __name__ == "__main__":
    sys.exit(main())
