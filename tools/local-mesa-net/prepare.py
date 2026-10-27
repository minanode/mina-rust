#!/usr/bin/env python3
"""Generate private test keys and an identical runtime config for both nodes."""

import argparse
import datetime
import json
import os
from pathlib import Path
import re
import secrets
import subprocess

from network_profile import fork_constants


GENESIS_WINNER = "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg"
SEEDS = (
    "2vaPBM2CkpnPhnPsrDEBwhX8d5XBP6UaedLCpVu3Y9bARWPCrQCC",
    "2vbu8qY9toPbkrgPEW2Wtd8oqHrbCvDnrvMQq1zvY6V4n3U6VasS",
)
PLACEHOLDER_LEDGER_HASH = "jwZEUpVHLxVCiSTjr6KkgsTXpj5QP3489aF2xFjcrSSdfthWm6v"


def add_prebuilt_ledger_hashes(config, ledger_tool, work):
    """Supply the epoch roots required by ledger-tool's prebuilt web config."""
    probe = json.loads(json.dumps(config))
    probe["ledger"]["hash"] = PLACEHOLDER_LEDGER_HASH
    path = work / "logs/ledger-hash-probe.json"
    path.write_text(json.dumps(probe) + "\n")
    result = subprocess.run([str(ledger_tool.resolve()), "--input", str(path),
                             "--output", str(work / "logs/ledger-hash-probe.bin")],
                            capture_output=True, text=True)
    path.unlink()
    match = re.search(r"ledger hashes don't match: (\w+) != (\w+)",
                      result.stdout + result.stderr)
    if not match or match.group(1) != PLACEHOLDER_LEDGER_HASH:
        raise RuntimeError("ledger-tool did not report the ledger root")
    root = match.group(2)
    config["ledger"]["hash"] = root
    for epoch in config["epoch_data"].values():
        epoch["hash"] = root


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--mina", type=Path, required=True)
    parser.add_argument("--ocaml", type=Path, required=True)
    parser.add_argument("--network", choices=("mainnet", "devnet"), default="mainnet")
    parser.add_argument("--genesis-delay", type=int, default=300)
    parser.add_argument("--ledger-tool", type=Path,
                        help="also write genesis.bin for a browser producer")
    args = parser.parse_args()
    os.umask(0o077)
    work = args.dir.resolve()
    work.mkdir(mode=0o700)  # Never overwrite an existing experiment or key.
    for name in ("keys", "tmp", "genesis", "logs"):
        (work / name).mkdir()
    password = secrets.token_urlsafe(32)
    (work / "keys/password").write_text(password)
    env = dict(os.environ, MINA_PRIVKEY_PASS=password, MINA_LIBP2P_PASS=password,
               TMPDIR=str(work / "tmp"), MINA_PROFILE=args.network)
    accounts = [{"pk": GENESIS_WINNER, "balance": "0.000001000",
                 "delegate": GENESIS_WINNER}]
    with (work / "logs/key-generation.log").open("w") as log:
        for name, balance in (("producer", "1000000"), ("sender", "1000"),
                              ("receiver", "1000"), ("small", "1000")):
            key = work / "keys" / name
            subprocess.run([str(args.mina.resolve()), "wallet", "generate",
                            "--output", str(key)], env=env, stdout=log,
                           stderr=subprocess.STDOUT, check=True)
            public_key = Path(str(key) + ".pub").read_text().strip()
            accounts.append({"pk": public_key, "balance": balance,
                             "delegate": public_key})
        for name in ("rust-p2p", "ocaml-p2p"):
            subprocess.run([str(args.ocaml.resolve()), "libp2p", "generate-keypair",
                            "--privkey-path", str(work / "keys" / name)],
                           env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
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
            "fork": fork_constants(args.network),
        },
        "ledger": {"accounts": accounts, "add_genesis_winner": False},
        "epoch_data": {name: {"accounts": accounts, "seed": seed}
                       for name, seed in zip(("staking", "next"), SEEDS)},
        "daemon": {"network_id": "p46c-" + secrets.token_hex(12),
                   "tx_pool_max_size": 3000},
    }
    if args.ledger_tool:
        add_prebuilt_ledger_hashes(config, args.ledger_tool, work)
    (work / "daemon.json").write_text(json.dumps(config, indent=2) + "\n")
    if args.ledger_tool:
        with (work / "logs/ledger-tool.log").open("w") as log:
            subprocess.run([str(args.ledger_tool.resolve()), "--input",
                            str(work / "daemon.json"), "--output",
                            str(work / "genesis.bin")], stdout=log,
                           stderr=subprocess.STDOUT, check=True)
        (work / "run.json").write_text(json.dumps({
            "genesis_timestamp": timestamp,
            "producer": accounts[1]["pk"],
            "network_id": config["daemon"]["network_id"],
            "ledger_hash": config["ledger"]["hash"],
        }, indent=2) + "\n")
    (work / "network.json").write_text(json.dumps({"network": args.network}) + "\n")
    print(f"Private run directory: {work}")
    print(f"Network profile: {args.network}")
    print(f"Genesis timestamp: {timestamp}")
    print(f"Producer public key: {accounts[1]['pk']}")


if __name__ == "__main__":
    main()
