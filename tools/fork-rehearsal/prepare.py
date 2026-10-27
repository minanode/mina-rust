#!/usr/bin/env python3
"""Generate throwaway keys and the old chain's runtime config for the R43f
hard-fork rehearsal: Mesa devnet constants and fork, a private ledger, early
stop slots and a scheduled next genesis.

The run directory keeps local-mesa-net's layout (keys/, tmp/, genesis/, logs/,
daemon.json, network.json), so its namespace, identity and stop helpers work on
it. The three genesis ledgers differ, so an export that took the wrong ledger
for an epoch would show in its hash.
"""

import argparse
import datetime
import json
import os
from pathlib import Path
import secrets
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "local-mesa-net"))
from network_profile import fork_constants  # noqa: E402

GENESIS_WINNER = "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg"
SEEDS = (
    "2vaPBM2CkpnPhnPsrDEBwhX8d5XBP6UaedLCpVu3Y9bARWPCrQCC",
    "2vbu8qY9toPbkrgPEW2Wtd8oqHrbCvDnrvMQq1zvY6V4n3U6VasS",
)
# name, balance (MINA), delegate (None: self)
KEYS = (
    ("producer", "3000000", None),        # Rust's block producer
    ("ocaml-producer", "1000000", None),  # OCaml's, on the new chain
    ("sender", "1000", None),
    ("receiver", "1000", None),
    ("small", "1000", None),
    ("delegator", "2000", "producer"),
    ("timed", "5000", None),
)


def add_ledger_hashes(config, mina, work):
    """Declare each ledger's root. OCaml 4.0.2 compares the staking and next
    epoch ledgers by their declared hashes and, if both are missing, uses the
    staking ledger as the next one (genesis_ledger_helper.ml L650-660)."""
    for name, ledger in (("genesis", config["ledger"]),
                         ("staking", config["epoch_data"]["staking"]),
                         ("next", config["epoch_data"]["next"])):
        path = work / "logs" / f"ledger-{name}.json"
        path.write_text(json.dumps(ledger["accounts"]) + "\n")
        out = subprocess.run([str(mina.resolve()), "ledger", "hash", "--ledger-file",
                              str(path), "--json"], capture_output=True, text=True, check=True)
        ledger["hash"] = json.loads(out.stdout)["ledger_hash"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--mina", type=Path, required=True)
    parser.add_argument("--ocaml", type=Path, required=True)
    parser.add_argument("--network", choices=("devnet",), default="devnet")
    parser.add_argument("--genesis-delay", type=int, default=420)
    parser.add_argument("--slot-tx-end", type=int, default=10)
    parser.add_argument("--slot-chain-end", type=int, default=14)
    parser.add_argument("--genesis-slot-delta", type=int, default=12)
    args = parser.parse_args()
    os.umask(0o077)
    work = args.dir.resolve()
    work.mkdir(mode=0o700)  # Never overwrite an earlier run or its keys.
    for name in ("keys", "tmp", "genesis", "logs"):
        (work / name).mkdir()
    password = secrets.token_urlsafe(32)
    (work / "keys/password").write_text(password)
    env = dict(os.environ, MINA_PRIVKEY_PASS=password, MINA_LIBP2P_PASS=password,
               TMPDIR=str(work / "tmp"), MINA_PROFILE=args.network)
    public = {}
    with (work / "logs/key-generation.log").open("w") as log:
        for name, _, _ in KEYS:
            key = work / "keys" / name
            # OCaml produces with a key from its own tool; the rest are ours.
            command = ([str(args.ocaml.resolve()), "advanced", "generate-keypair",
                        "--privkey-path", str(key)] if name == "ocaml-producer" else
                       [str(args.mina.resolve()), "wallet", "generate", "--output", str(key)])
            subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
            public[name] = Path(str(key) + ".pub").read_text().strip()
        for name in ("rust-p2p", "ocaml-p2p"):
            subprocess.run([str(args.ocaml.resolve()), "libp2p", "generate-keypair",
                            "--privkey-path", str(work / "keys" / name)],
                           env=env, stdout=log, stderr=subprocess.STDOUT, check=True)

    def account(name, balance, delegate):
        row = {"pk": public[name], "balance": balance,
               "delegate": public[delegate or name]}
        if name == "timed":
            # The cliff is a slot since genesis after the Mesa devnet fork's
            # 859560, so the account is still locked at the rehearsal's fork.
            row["timing"] = {"initial_minimum_balance": "4000", "cliff_time": "860000",
                             "cliff_amount": "1000", "vesting_period": "10",
                             "vesting_increment": "100"}
        return row

    winner = {"pk": GENESIS_WINNER, "balance": "0.000001", "delegate": GENESIS_WINNER}
    genesis = [winner] + [account(*row) for row in KEYS]
    # Different balances, so each ledger has its own root.
    staking = [dict(row) for row in genesis]
    staking[5] = dict(staking[5], balance="900")       # small
    nxt = [dict(row) for row in genesis]
    nxt[4] = dict(nxt[4], balance="1100")              # receiver
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
        "ledger": {"accounts": genesis, "add_genesis_winner": False},
        "epoch_data": {
            "staking": {"accounts": staking, "seed": SEEDS[0]},
            "next": {"accounts": nxt, "seed": SEEDS[1]},
        },
        "daemon": {
            "network_id": "r43f-" + secrets.token_hex(12),
            "txpool_max_size": 3000,
            "slot_tx_end": args.slot_tx_end,
            "slot_chain_end": args.slot_chain_end,
            "hard_fork_genesis_slot_delta": args.genesis_slot_delta,
        },
    }
    add_ledger_hashes(config, args.mina, work)
    (work / "daemon.json").write_text(json.dumps(config, indent=2) + "\n")
    (work / "network.json").write_text(json.dumps({"network": args.network}) + "\n")
    (work / "run.json").write_text(json.dumps({
        "genesis_timestamp": timestamp,
        "accounts": public,
        "stops": config["daemon"],
    }, indent=2) + "\n")
    print(f"Run directory: {work}")
    print(f"Old genesis: {timestamp}; slot_tx_end {args.slot_tx_end}, slot_chain_end "
          f"{args.slot_chain_end}, next genesis at slot "
          f"{args.slot_chain_end + args.genesis_slot_delta}")


if __name__ == "__main__":
    main()
