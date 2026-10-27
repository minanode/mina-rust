#!/usr/bin/env python3
"""Make a private Mesa chain for a web node to produce on, alone (I34c).

Writes a fresh run directory, never an existing one: a throwaway encrypted
producer key and its random password under keys/ (0700/0600, never printed),
daemon.json with public accounts only, and genesis.bin, the prebuilt config
the web node loads. The producer holds 1,000,000 MINA of about 1,002,000, so
it wins about three slots in four. Constants are Mesa mainnet's, as in
P4.6c's private network (tools/local-mesa-net/prepare.py on its branch):
same fork, same circuits and keys, a distinct genesis and so a distinct
chain ID. No peer, seed or public network is configured anywhere.
"""

import argparse
import datetime
import json
import os
from pathlib import Path
import re
import secrets
import subprocess

GENESIS_WINNER = "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg"
# OCaml 4.0.2 genesis_ledgers/mainnet.json; changing this changes the circuits.
FORK = {
    "state_hash": "3NKHyxzgM9z4C1wUz5ZhJBmNhtDzDQGr1PGvWRwiCUcFvsdTco2T",
    "blockchain_length": 548146,
    "global_slot_since_genesis": 958440,
}
SEEDS = (
    "2vaPBM2CkpnPhnPsrDEBwhX8d5XBP6UaedLCpVu3Y9bARWPCrQCC",
    "2vbu8qY9toPbkrgPEW2Wtd8oqHrbCvDnrvMQq1zvY6V4n3U6VasS",
)
# Any valid ledger hash: P4.6c's corrected run's genesis ledger.
PLACEHOLDER_LEDGER_HASH = "jwZEUpVHLxVCiSTjr6KkgsTXpj5QP3489aF2xFjcrSSdfthWm6v"


def ledger_hash(ledger_tool, config, work):
    """The genesis ledger's root hash, as ledger-tool computes it.

    A prebuilt config needs each epoch ledger's hash in daemon.json, and
    ledger-tool has no command that only prints it. It does check a declared
    genesis ledger hash, before anything else, and names the computed one
    when they differ: declare a placeholder and read the answer.
    """
    probe = json.loads(json.dumps(config))
    probe["ledger"]["hash"] = PLACEHOLDER_LEDGER_HASH
    probe_path = work / "logs/ledger-hash-probe.json"
    probe_path.write_text(json.dumps(probe, indent=2) + "\n")
    result = subprocess.run([str(ledger_tool.resolve()), "--input",
                             str(probe_path), "--output",
                             str(work / "logs/ledger-hash-probe.bin")],
                            capture_output=True, text=True)
    probe_path.unlink()
    match = re.search(r"ledger hashes don't match: (\w+) != (\w+)",
                      result.stdout + result.stderr)
    if not match or match.group(1) != PLACEHOLDER_LEDGER_HASH:
        raise SystemExit("ledger-tool did not report the genesis ledger "
                         "hash:\n" + result.stdout + result.stderr)
    return match.group(2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--mina", type=Path, required=True,
                        help="a mina CLI, for `wallet generate`")
    parser.add_argument("--ledger-tool", type=Path, required=True)
    parser.add_argument("--genesis-delay", type=int, default=420,
                        help="seconds from now to the genesis timestamp")
    args = parser.parse_args()
    os.umask(0o077)
    work = args.dir.resolve()
    work.mkdir(mode=0o700)  # Never overwrite an experiment or its key.
    for name in ("keys", "logs"):
        (work / name).mkdir()
    password = secrets.token_urlsafe(32)
    (work / "keys/password").write_text(password)
    env = dict(os.environ, MINA_PRIVKEY_PASS=password)
    accounts = [{"pk": GENESIS_WINNER, "balance": "0.000001000",
                 "delegate": GENESIS_WINNER}]
    with (work / "logs/key-generation.log").open("w") as log:
        for name, balance in (("producer", "1000000"), ("small", "1000"),
                              ("other", "1000")):
            key = work / "keys" / name
            subprocess.run([str(args.mina.resolve()), "wallet", "generate",
                            "--network", "mainnet", "--output", str(key)],
                           env=env, stdout=log, stderr=subprocess.STDOUT,
                           check=True)
            public_key = Path(str(key) + ".pub").read_text().strip()
            accounts.append({"pk": public_key, "balance": balance,
                             "delegate": public_key})
    timestamp = (datetime.datetime.now(datetime.timezone.utc)
                 + datetime.timedelta(seconds=args.genesis_delay))
    timestamp = timestamp.replace(microsecond=0).isoformat().replace("+00:00", "Z")
    config = {
        "genesis": {
            "genesis_state_timestamp": timestamp,
            "k": 290, "delta": 0, "slots_per_epoch": 7140,
            "slots_per_sub_window": 7, "grace_period_slots": 2160,
        },
        "proof": {
            "level": "full", "sub_windows_per_window": 11, "ledger_depth": 35,
            "work_delay": 2, "block_window_duration_ms": 90000,
            "transaction_capacity": {"2_to_the": 7}, "coinbase_amount": "360",
            "supercharged_coinbase_factor": 1, "account_creation_fee": "1",
            "fork": FORK,
        },
        "ledger": {"accounts": accounts, "add_genesis_winner": False},
        "epoch_data": {name: {"accounts": accounts, "seed": seed}
                       for name, seed in zip(("staking", "next"), SEEDS)},
        "daemon": {"network_id": "i34c-" + secrets.token_hex(12),
                   "tx_pool_max_size": 3000},
    }
    # All three ledgers hold the same accounts, so they share one hash.
    root = ledger_hash(args.ledger_tool, config, work)
    config["ledger"]["hash"] = root
    for epoch in config["epoch_data"].values():
        epoch["hash"] = root
    (work / "daemon.json").write_text(json.dumps(config, indent=2) + "\n")
    with (work / "logs/ledger-tool.log").open("w") as log:
        subprocess.run([str(args.ledger_tool.resolve()), "--input",
                        str(work / "daemon.json"), "--output",
                        str(work / "genesis.bin")],
                       stdout=log, stderr=subprocess.STDOUT, check=True)
    (work / "run.json").write_text(json.dumps({
        "genesis_timestamp": timestamp,
        "producer": accounts[1]["pk"],
        "network_id": config["daemon"]["network_id"],
        "ledger_hash": root,
    }, indent=2) + "\n")
    print(f"Private run directory: {work}")
    print(f"Genesis timestamp: {timestamp}")
    print(f"Producer public key: {accounts[1]['pk']}")


if __name__ == "__main__":
    main()
