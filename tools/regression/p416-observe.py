#!/usr/bin/env python3
"""Record read-only relay and independent public GraphQL payment evidence."""

import argparse
import json
import subprocess
import time
from datetime import datetime, timezone

SENDER = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV"
RECIPIENT = "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX"
QUERY = """query {
  syncStatus
  sender: account(publicKey: "%s") { nonce balance { total } }
  recipient: account(publicKey: "%s") { nonce balance { total } }
  pooledUserCommands { hash }
  bestChain(maxLength: 20) {
    stateHash creator
    protocolState { previousStateHash consensusState { blockHeight } }
    transactions {
      userCommands { hash failureReason nonce amount fee memo }
    }
  }
}""" % (SENDER, RECIPIENT)


def sample(url, wanted_hash):
    result = subprocess.run(
        [
            "curl",
            "--fail",
            "--silent",
            "--show-error",
            "--max-time",
            "15",
            url,
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
        ],
        input=json.dumps({"query": QUERY}),
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode:
        return {"error": f"curl exit {result.returncode}: {result.stderr.strip()}"}
    reply = json.loads(result.stdout)
    if reply.get("errors"):
        return {"errors": reply["errors"]}
    data = reply["data"]
    return {
        "sync": data["syncStatus"],
        "sender": data["sender"],
        "recipient": data["recipient"],
        "pool": [tx["hash"] for tx in data["pooledUserCommands"] or []],
        "chain": [
            {
                "hash": block["stateHash"],
                "parent": block["protocolState"]["previousStateHash"],
                "height": int(block["protocolState"]["consensusState"]["blockHeight"]),
                "creator": block["creator"],
                "commands": [
                    cmd
                    for cmd in block["transactions"]["userCommands"]
                    if cmd["hash"] == wanted_hash
                ],
            }
            for block in data["bestChain"] or []
        ],
    }


def inclusion(view):
    return next((block for block in view.get("chain", []) if block["commands"]), None)


def has_descendant(view, block):
    return any(
        candidate["parent"] == block["hash"] for candidate in view.get("chain", [])
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hash", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--duration", type=int, default=1800)
    parser.add_argument("--interval", type=float, default=3)
    args = parser.parse_args()
    end = time.monotonic() + args.duration
    relay_url = "http://127.0.0.1:3540/graphql"
    public_url = "https://mina-devnet-graphql.aurowallet.com/graphql"
    next_public = 0
    last_public = None
    reported = set()
    with open(args.output, "a", encoding="utf8", buffering=1) as output:
        while time.monotonic() < end:
            row = {"at": datetime.now(timezone.utc).isoformat()}
            row["relay"] = sample(relay_url, args.hash)
            if time.monotonic() >= next_public:
                last_public = sample(public_url, args.hash)
                row["public"] = last_public
                next_public = time.monotonic() + 15
            output.write(json.dumps(row, separators=(",", ":")) + "\n")
            for name, view in (("relay", row["relay"]), ("public", last_public or {})):
                if args.hash in view.get("pool", []) and (name, "pool") not in reported:
                    print(f"{row['at']} {name} pool", flush=True)
                    reported.add((name, "pool"))
                block = inclusion(view)
                if block and (name, "included") not in reported:
                    print(
                        f"{row['at']} {name} included height={block['height']} "
                        f"block={block['hash']} failure={block['commands'][0]['failureReason']}",
                        flush=True,
                    )
                    reported.add((name, "included"))
                if block and has_descendant(view, block) and (name, "descendant") not in reported:
                    print(f"{row['at']} {name} direct descendant", flush=True)
                    reported.add((name, "descendant"))
            if ("relay", "descendant") in reported and ("public", "descendant") in reported:
                break
            time.sleep(min(args.interval, max(0, end - time.monotonic())))


if __name__ == "__main__":
    main()
