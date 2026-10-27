#!/usr/bin/env python3
"""Read-only R39 pool, application and canonical-descendant observations."""

import argparse
import json
import subprocess
import time
from datetime import datetime, timezone

SENDER = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV"
RECIPIENT = "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX"
QUERY = '''query {
  syncStatus
  sender: account(publicKey: "%s") { nonce balance { total } }
  recipient: account(publicKey: "%s") { nonce balance { total } }
  pooledUserCommands { hash }
  pooledZkappCommands { hash }
  bestChain(maxLength: 20) {
    stateHash creator
    protocolState { previousStateHash consensusState { blockHeight } }
    transactions {
      userCommands { hash failureReason nonce amount fee memo }
      zkappCommands { hash failureReason { index failures } }
    }
  }
}''' % (SENDER, RECIPIENT)


def now():
    return datetime.now(timezone.utc).isoformat()


def sample(url):
    started = now()
    # Auro rejects urllib's request with HTTP 403 on this machine; curl is
    # also the transport used by the P4.16 public observer.
    result = subprocess.run(
        ["curl", "--fail", "--silent", "--show-error", "--max-time", "15", url,
         "-H", "Content-Type: application/json", "--data-binary", "@-"],
        input=json.dumps({"query": QUERY}), text=True, capture_output=True,
        check=False,
    )
    if result.returncode:
        raise ValueError(f"read-only curl request failed: exit {result.returncode}")
    reply = json.loads(result.stdout)
    return {"requestAt": started, "responseAt": now(), **reply}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--relay", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--duration", type=int, default=1800)
    args = parser.parse_args()
    deadline = time.monotonic() + args.duration
    next_public = 0
    with open(args.output, "a", encoding="utf8", buffering=1) as output:
        while time.monotonic() < deadline:
            endpoints = {"relay": args.relay, "devnetRelay": "http://127.0.0.1:3540/graphql"}
            if time.monotonic() >= next_public:
                endpoints["public"] = "https://mina-devnet-graphql.aurowallet.com/graphql"
                next_public = time.monotonic() + 15
            for name, url in endpoints.items():
                try:
                    row = {"source": name, **sample(url)}
                except (OSError, ValueError) as error:
                    row = {"source": name, "responseAt": now(), "error": str(error)}
                output.write(json.dumps(row, separators=(",", ":")) + "\n")
            time.sleep(min(3, max(0, deadline - time.monotonic())))


if __name__ == "__main__":
    main()
