#!/usr/bin/env python3
"""Sample P4.15 zkApp pool, chain, and account evidence from two GraphQL nodes."""

import argparse
import json
import time
import urllib.request
from datetime import datetime, timezone


SENDER = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV"
RECIPIENT = "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX"
QUERY = """query {
  syncStatus
  sender: account(publicKey: "%s") { nonce balance { total } }
  recipient: account(publicKey: "%s") { nonce balance { total } }
  pooledZkappCommands { hash }
  bestChain(maxLength: 20) {
    stateHash creator
    protocolState { previousStateHash consensusState { blockHeight } }
    transactions { zkappCommands { hash failureReason { index failures } } }
  }
}""" % (SENDER, RECIPIENT)


def sample(url, wanted_hash):
    request = urllib.request.Request(
        url,
        data=json.dumps({"query": QUERY}).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        reply = json.load(response)
    if reply.get("errors"):
        return {"errors": reply["errors"]}
    data = reply["data"]
    return {
        "sync": data["syncStatus"],
        "sender": data["sender"],
        "recipient": data["recipient"],
        "pool": [z["hash"] for z in data["pooledZkappCommands"]],
        "chain": [
            {
                "hash": block["stateHash"],
                "parent": block["protocolState"]["previousStateHash"],
                "height": int(block["protocolState"]["consensusState"]["blockHeight"]),
                "creator": block["creator"],
                "zkapps": [
                    z
                    for z in block["transactions"]["zkappCommands"]
                    if z["hash"] == wanted_hash
                ],
            }
            for block in data["bestChain"]
        ],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hash", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--duration", type=int, default=1800)
    parser.add_argument("--interval", type=float, default=3)
    args = parser.parse_args()
    deadline = time.monotonic() + args.duration
    with open(args.output, "a", encoding="utf-8", buffering=1) as output:
        while time.monotonic() < deadline:
            row = {"at": datetime.now(timezone.utc).isoformat()}
            for name, url in (
                ("node", "http://127.0.0.1:3480/graphql"),
                ("relay", "http://127.0.0.1:3540/graphql"),
            ):
                try:
                    row[name] = sample(url, args.hash)
                except (OSError, ValueError, KeyError) as error:
                    row[name] = {"error": str(error)}
            output.write(json.dumps(row, separators=(",", ":")) + "\n")
            time.sleep(min(args.interval, max(0, deadline - time.monotonic())))


if __name__ == "__main__":
    main()
