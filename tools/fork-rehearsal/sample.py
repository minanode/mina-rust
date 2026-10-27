#!/usr/bin/env python3
"""Every ten seconds, record both nodes' best chains, sync status and chain
ids over GraphQL, in `samples-<phase>.jsonl`. Run through enter.sh."""

import argparse
import datetime
import json
import os
from pathlib import Path
import time
import urllib.request

QUERY = """{ syncStatus daemonStatus { chainId peers { peerId } }
  genesisBlock { stateHash }
  bestChain(maxLength: 40) { stateHash creatorAccount { publicKey }
    protocolState { previousStateHash
      blockchainState { stagedLedgerHash snarkedLedgerHash }
      consensusState { blockHeight slot slotSinceGenesis epoch } }
    transactions { userCommands { hash } coinbase } } }"""


def graphql(port, query):
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}/graphql", data=json.dumps({"query": query}).encode(),
        headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=20) as response:
        return json.load(response)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--phase", choices=("old", "new"), required=True)
    parser.add_argument("--seconds", type=int, default=3600)
    args = parser.parse_args()
    work = args.dir.resolve()
    assert os.readlink("/proc/self/ns/net") == json.loads(
        (work / "namespace.json").read_text())["netns"], "run through enter.sh"
    (work / f"sample-{args.phase}.pid").write_text(str(os.getpid()) + "\n")
    out = work / f"samples-{args.phase}.jsonl"
    stop = time.time() + args.seconds
    while time.time() < stop:
        row = {"time": datetime.datetime.now(datetime.timezone.utc).isoformat()}
        for name, port in (("rust", 3660), ("ocaml", 3670)):
            try:
                row[name] = graphql(port, QUERY)
            except Exception as error:  # a node may be starting or stopped
                row[name] = {"sample_error": str(error)}
        with out.open("a") as f:
            f.write(json.dumps(row) + "\n")
        time.sleep(10)


if __name__ == "__main__":
    main()
