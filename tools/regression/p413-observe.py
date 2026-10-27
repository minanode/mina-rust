#!/usr/bin/env python3
"""Sample public GraphQL payment evidence without reading wallet material."""

import argparse
import json
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone


SENDER = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV"
RECEIVER = "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX"
QUERY = """query {
  syncStatus
  account(publicKey: "%s") { nonce balance { total } }
  recipient: account(publicKey: "%s") { nonce balance { total } }
  pooledUserCommands { hash }
  bestChain(maxLength: 20) {
    stateHash creator
    protocolState { previousStateHash consensusState { blockHeight } }
    transactions { userCommands { hash } }
  }
}""" % (SENDER, RECEIVER)


def graphql(url):
    body = json.dumps({"query": QUERY}).encode()
    request = urllib.request.Request(
        url, data=body, headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(request, timeout=10) as response:
        result = json.load(response)
    if result.get("errors"):
        return {"errors": result["errors"]}
    data = result["data"]
    return {
        "sync": data.get("syncStatus"),
        "sender": data.get("account"),
        "recipient": data.get("recipient"),
        "pool": [command["hash"] for command in data.get("pooledUserCommands") or []],
        "chain": [
            {
                "hash": block["stateHash"],
                "parent": block["protocolState"]["previousStateHash"],
                "height": int(block["protocolState"]["consensusState"]["blockHeight"]),
                "creator": block["creator"],
                "commands": [
                    command["hash"]
                    for command in block["transactions"]["userCommands"]
                ],
            }
            for block in data.get("bestChain") or []
        ],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True)
    parser.add_argument("--node", default="http://127.0.0.1:3470/graphql")
    parser.add_argument("--relay", default="http://127.0.0.1:3540/graphql")
    parser.add_argument("--duration", type=int, default=7200)
    parser.add_argument("--interval", type=float, default=3)
    args = parser.parse_args()
    end = time.monotonic() + args.duration
    with open(args.output, "a", encoding="utf-8", buffering=1) as output:
        while time.monotonic() < end:
            sample = {}
            for name, url in (("node", args.node), ("relay", args.relay)):
                try:
                    sample[name] = graphql(url)
                except (OSError, ValueError, KeyError, urllib.error.URLError) as error:
                    sample[name] = {"error": str(error)}
            sample["at"] = datetime.now(timezone.utc).isoformat()
            output.write(json.dumps(sample, separators=(",", ":")) + "\n")
            time.sleep(min(args.interval, max(0, end - time.monotonic())))


if __name__ == "__main__":
    main()
