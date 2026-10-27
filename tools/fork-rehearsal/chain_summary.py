#!/usr/bin/env python3
"""Summarise a phase's samples: every block either node had in its best
chain, with creator, slot, commands, coinbase, and when each node first had
it there; and whether the final tips agree."""

import argparse
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--phase", choices=("old", "new"), required=True)
    args = parser.parse_args()
    work = args.dir.resolve()
    run = json.loads((work / "run.json").read_text())
    names = {pk: name for name, pk in run["accounts"].items()}
    blocks, first_seen, tips = {}, {}, {}
    for line in (work / f"samples-{args.phase}.jsonl").read_text().splitlines():
        row = json.loads(line)
        for node in ("rust", "ocaml"):
            data = (row.get(node) or {}).get("data") or {}
            chain = data.get("bestChain") or []
            if chain:
                tips[node] = {"time": row["time"], "state_hash": chain[-1]["stateHash"],
                              "height": int(chain[-1]["protocolState"]["consensusState"]["blockHeight"]),
                              "genesis": (data.get("genesisBlock") or {}).get("stateHash"),
                              "chain_id": (data.get("daemonStatus") or {}).get("chainId")}
            for block in chain:
                cs = block["protocolState"]["consensusState"]
                blocks[block["stateHash"]] = {
                    "height": int(cs["blockHeight"]), "slot": int(cs["slot"]),
                    "slot_since_genesis": int(cs["slotSinceGenesis"]),
                    "creator": names.get(block["creatorAccount"]["publicKey"],
                                         block["creatorAccount"]["publicKey"]),
                    "user_commands": len(block["transactions"]["userCommands"]),
                    "coinbase": block["transactions"]["coinbase"],
                    "staged_ledger_hash": block["protocolState"]["blockchainState"]["stagedLedgerHash"],
                }
                first_seen.setdefault((block["stateHash"], node), row["time"])
    rows = []
    for state_hash, block in sorted(blocks.items(), key=lambda kv: kv[1]["height"]):
        block = dict(block, state_hash=state_hash,
                     rust_first_seen=first_seen.get((state_hash, "rust")),
                     ocaml_first_seen=first_seen.get((state_hash, "ocaml")))
        rows.append(block)
    summary = {"phase": args.phase, "blocks": rows, "final_tips": tips,
               "tips_agree": len({t["state_hash"] for t in tips.values()}) == 1 and len(tips) == 2}
    out = work / f"chain-{args.phase}.json"
    out.write_text(json.dumps(summary, indent=2) + "\n")
    for row in rows:
        print(row["height"], row["slot"], row["state_hash"][:16], row["creator"],
              row["user_commands"], row["coinbase"], (row["rust_first_seen"] or "-")[11:19],
              (row["ocaml_first_seen"] or "-")[11:19])
    print("tips agree:", summary["tips_agree"], {k: v["state_hash"][:16] for k, v in tips.items()})


if __name__ == "__main__":
    main()
