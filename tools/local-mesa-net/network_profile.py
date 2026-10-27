"""One persisted network profile for config, identity, daemons and signatures."""

import json
from pathlib import Path


def fork_constants(network):
    assert network in ("mainnet", "devnet"), "unknown network profile"
    # OCaml 4.0.2 runtime proof.fork; these also select the circuit dump.
    root = Path(__file__).resolve().parents[2]
    path = root / "tools/mesa-circuits" / f"mesa-{network}-fork.json"
    return json.loads(path.read_text())["proof"]["fork"]


def load_profile(work):
    path = work / "network.json"
    # Runs prepared before profile support used mainnet exclusively.
    network = json.loads(path.read_text())["network"] if path.exists() else "mainnet"
    config = json.loads((work / "daemon.json").read_text())
    assert config["proof"]["fork"] == fork_constants(network), "profile/fork mismatch"
    return network
