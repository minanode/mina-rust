#!/usr/bin/env python3
"""At the old chain's stop, have each node write the next chain's config from
its own state, and time it. Run through enter.sh.

- Rust: `mina fork export --verify` into fork/rust/.
- OCaml: `mina advanced generate-hardfork-config` (fork validation on) into
  fork/ocaml/, and GraphQL `fork_config` (the legacy all-accounts config) into
  fork/ocaml-graphql-fork-config.json.
"""

import argparse
import datetime
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.request


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def run(command, env, log):
    started = time.time()
    with log.open("w") as out:
        result = subprocess.run(command, env=env, stdout=out, stderr=subprocess.STDOUT,
                                timeout=1800)
    return {"returncode": result.returncode, "seconds": round(time.time() - started, 3),
            "log": str(log)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dir", type=Path, required=True)
    parser.add_argument("--mina", type=Path, required=True)
    args = parser.parse_args()
    work = args.dir.resolve()
    assert os.readlink("/proc/self/ns/net") == json.loads(
        (work / "namespace.json").read_text())["netns"], "run through enter.sh"
    root = Path(__file__).resolve().parents[2]
    network = json.loads((work / "network.json").read_text())["network"]
    fork = work / "fork"
    fork.mkdir()
    base_env = {k: v for k, v in os.environ.items() if not k.startswith(("MINA_", "OCAML_"))}
    base_env.update(TMPDIR=str(work / "tmp"), MINA_PROFILE=network)
    record = {"started": now()}

    record["rust"] = run([str(args.mina.resolve()), "--network", network, "fork", "export",
                          "--node", "http://127.0.0.1:3660", "--out", str(fork / "rust"),
                          "--verify"], base_env, work / "logs/fork-export-rust.log")
    record["rust"]["finished"] = now()

    record["ocaml_advanced"] = run(
        [str(root / "scripts/ocaml-mina"), "advanced", "generate-hardfork-config",
         "--hardfork-config-dir", str(fork / "ocaml"),
         "--generate-fork-validation", "true", "--daemon-port", "127.0.0.1:8871"],
        base_env, work / "logs/fork-export-ocaml-advanced.log")
    record["ocaml_advanced"]["finished"] = now()

    started = time.time()
    request = urllib.request.Request(
        "http://127.0.0.1:3670/graphql", data=json.dumps({"query": "{ fork_config }"}).encode(),
        headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=1800) as response:
            answer = json.load(response)
        (fork / "ocaml-graphql-fork-config.json").write_text(
            json.dumps(answer.get("data", {}).get("fork_config")) + "\n")
        record["ocaml_graphql"] = {"errors": answer.get("errors"),
                                   "seconds": round(time.time() - started, 3)}
    except Exception as error:
        record["ocaml_graphql"] = {"error": str(error)}
    record["ocaml_graphql"]["finished"] = now()
    (work / "logs/fork-export.json").write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps(record, indent=2))


if __name__ == "__main__":
    main()
