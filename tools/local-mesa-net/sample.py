#!/usr/bin/env python3
"""Capture block acceptance evidence from the two local node APIs."""

import argparse
import datetime
import json
import os
from pathlib import Path
import time
import urllib.request

from processes import track


QUERY = """{ syncStatus daemonStatus { blockchainLength chainId }
  genesisBlock { stateHash }
  bestChain(maxLength: 100) { stateHash protocolState {
    previousStateHash consensusState { blockHeight slotSinceGenesis }
  } transactions { userCommands { hash } } }
}"""


def fetch(url, query=None):
    try:
        request = urllib.request.Request(url)
        if query is not None:
            request.data = json.dumps({"query": query}).encode()
            request.add_header("Content-Type", "application/json")
        with urllib.request.urlopen(request, timeout=5) as response:
            return json.load(response)
    except Exception as error:
        return {"sample_error": str(error)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--rust-port", type=int, default=3660)
    parser.add_argument("--ocaml-port", type=int, default=3670)
    parser.add_argument("--seconds", type=int, default=3600)
    args = parser.parse_args()
    (args.dir / "sample.pid").write_text(str(os.getpid()) + "\n")
    end = time.monotonic() + args.seconds
    with (args.dir / "samples.jsonl").open("a", buffering=1) as output:
        while time.monotonic() < end:
            track(args.dir.resolve())
            rust = f"http://127.0.0.1:{args.rust_port}"
            ocaml = f"http://127.0.0.1:{args.ocaml_port}"
            row = {
                "time": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                "rust_status": fetch(rust + "/status"),
                "producer": fetch(rust + "/stats/block_producer"),
                "rust_peers": fetch(rust + "/state/peers"),
                "ocaml": fetch(ocaml + "/graphql", QUERY),
            }
            output.write(json.dumps(row) + "\n")
            time.sleep(10)


if __name__ == "__main__":
    main()
