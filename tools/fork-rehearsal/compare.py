#!/usr/bin/env python3
"""Compare the next-chain configs the two nodes wrote at the stop, field by
field and ledger by ledger, and run OCaml's own tools on Rust's export.

Writes fork/comparison.json. OCaml commands use the run's private TMPDIR;
none of them talks to a network.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def load(path):
    return json.loads(Path(path).read_text())


def get(config, path):
    for key in path.split("."):
        if not isinstance(config, dict) or key not in config:
            return None
        config = config[key]
    return config


FIELDS = (
    "genesis.genesis_state_timestamp",
    "proof.fork.state_hash",
    "proof.fork.blockchain_length",
    "proof.fork.global_slot_since_genesis",
    "ledger.hash",
    "ledger.add_genesis_winner",
    "epoch_data.staking.seed",
    "epoch_data.staking.hash",
    "epoch_data.next.seed",
    "epoch_data.next.hash",
)


def compare_fields(rust, ocaml, fields=FIELDS):
    rows = []
    for field in fields:
        a, b = get(rust, field), get(ocaml, field)
        rows.append({"field": field, "rust": a, "ocaml": b, "equal": a == b})
    return rows


def ocaml(work, args, log, env_extra=None):
    env = {k: v for k, v in os.environ.items() if not k.startswith(("MINA_", "OCAML_"))}
    env.update(TMPDIR=str(work / "tmp"), MINA_PROFILE=load(work / "network.json")["network"])
    env.update(env_extra or {})
    with open(log, "w") as out:
        result = subprocess.run([str(ROOT / "scripts/ocaml-mina"), *args], env=env,
                                stdout=out, stderr=subprocess.STDOUT, timeout=3600)
    return result.returncode, Path(log).read_text()


def chain_id(work, configs, tag):
    args = ["internal", "chain-id"]
    for config in configs:
        args += ["--config-file", str(config)]
    args.append("--from-config-hashes-only")
    code, text = ocaml(work, args, work / f"logs/chain-id-{tag}.log")
    found = re.findall(r"[0-9a-f]{64}", text)
    return {"returncode": code, "chain_id": found[-1] if found else None}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    args = parser.parse_args()
    work = args.dir.resolve()
    fork = work / "fork"
    rust = fork / "rust"
    ocaml_dir = fork / "ocaml"
    result = {}

    # The hash-only configs: Rust's stable/migrated against OCaml's
    # fork_validation/legacy (stable) and main (migrated) daemon.json.
    result["stable"] = compare_fields(load(rust / "daemon.json"),
                                      load(ocaml_dir / "fork_validation/legacy/daemon.json"))
    result["migrated"] = compare_fields(load(rust / "daemon-migrated.json"),
                                        load(ocaml_dir / "daemon.json"))
    result["ocaml_only_fields"] = {
        "s3_data_hash": {name: get(load(path), "ledger.s3_data_hash") for name, path in (
            ("main", ocaml_dir / "daemon.json"),
            ("legacy", ocaml_dir / "fork_validation/legacy/daemon.json"))},
        "tarballs": sorted(p.name for p in (ocaml_dir / "genesis").glob("*.tar.gz")),
        "legacy_tarballs": sorted(
            p.name for p in (ocaml_dir / "fork_validation/legacy/genesis").glob("*.tar.gz")),
        "activated": (ocaml_dir / "activated").exists(),
    }

    # The all-accounts configs: Rust's fork_config.json against OCaml's
    # GraphQL fork_config, account by account.
    ours = load(rust / "fork_config.json")
    theirs = load(fork / "ocaml-graphql-fork-config.json")
    # GraphQL `fork_config` has no epoch ledger hashes; ours declares them
    # (OCaml would otherwise load the staking ledger as the next one), so
    # those two are checked against OCaml's legacy daemon.json instead.
    result["fork_config_fields"] = compare_fields(
        ours, theirs, [f for f in FIELDS if not f.startswith("epoch_data.") or
                       not f.endswith(".hash")])
    legacy = load(ocaml_dir / "fork_validation/legacy/daemon.json")
    result["fork_config_fields"] += [
        {"field": f + " (vs legacy daemon.json)", "rust": get(ours, f),
         "ocaml": get(legacy, f), "equal": get(ours, f) == get(legacy, f)}
        for f in ("epoch_data.staking.hash", "epoch_data.next.hash")]
    accounts = {}
    for name, path in (("ledger", "ledger.accounts"), ("staking", "epoch_data.staking.accounts"),
                       ("next", "epoch_data.next.accounts")):
        a, b = get(ours, path) or [], get(theirs, path) or []
        differing = [i for i, (x, y) in enumerate(zip(a, b)) if x != y]
        accounts[name] = {"rust": len(a), "ocaml": len(b),
                          "equal_in_order": a == b, "differing_indexes": differing[:20]}
    result["accounts"] = accounts
    result["fork_config_key_sets_equal"] = sorted(ours) == sorted(theirs or {})

    # OCaml's own tools on Rust's export.
    check = fork / "check"
    check.mkdir(exist_ok=True)
    code, _ = ocaml(work, ["--config-file", str(rust / "fork_config.json"),
                           "--genesis-dir", str(check / "rust-genesis"),
                           "--hash-output-file", str(check / "rust-hashes.json")],
                    work / "logs/create-genesis-rust.log",
                    {"OCAML_MINA_BIN": "mina-create-genesis"})
    created = load(check / "rust-hashes.json") if code == 0 else None
    result["mina_create_genesis_on_rust_export"] = {
        "returncode": code, "hashes": created,
        "equal_to_rust": created is not None and all(
            get(created, f) == get(load(rust / "daemon.json"), f)
            for f in ("ledger.hash", "epoch_data.staking.hash", "epoch_data.next.hash")),
    }
    base = work / "new/ocaml-base.json"
    result["chain_id_from_hashes"] = {
        "rust_daemon_json": chain_id(work, [base, rust / "daemon.json"], "rust"),
        "ocaml_legacy_daemon_json": chain_id(
            work, [base, ocaml_dir / "fork_validation/legacy/daemon.json"], "ocaml-legacy"),
    }
    result["files"] = {
        str(p.relative_to(fork)): {"bytes": p.stat().st_size,
                                    "sha256": hashlib.sha256(p.read_bytes()).hexdigest()}
        for p in sorted(fork.rglob("*")) if p.is_file() and "check" not in p.parts
        and not p.name.endswith(".tar.gz")
    }
    result["all_equal"] = (
        all(row["equal"] for key in ("stable", "migrated", "fork_config_fields")
            for row in result[key])
        and all(row["equal_in_order"] for row in accounts.values())
        and result["mina_create_genesis_on_rust_export"]["equal_to_rust"]
        and result["chain_id_from_hashes"]["rust_daemon_json"]["chain_id"]
        == result["chain_id_from_hashes"]["ocaml_legacy_daemon_json"]["chain_id"]
    )
    (fork / "comparison.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({k: result[k] for k in ("accounts", "all_equal")}, indent=2))
    for key in ("stable", "migrated", "fork_config_fields"):
        for row in result[key]:
            print(f"{key:20} {row['field']:40} {'=' if row['equal'] else 'DIFFERENT'}")


if __name__ == "__main__":
    main()
