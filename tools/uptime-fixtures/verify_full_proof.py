#!/usr/bin/env python3
"""Run the pinned OCaml delegation verifier, with explicit acceptance checks.

MINA_PROFILE=devnet and a private TMPDIR must be set by the caller. This tool
does not contact an uptime service. The reference verifier may download public
genesis ledgers/verification keys while initializing the supplied config.
"""

import argparse
import base64
import copy
import datetime
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

RESULT_FIELDS = ("state_hash", "parent", "height", "slot", "verified", "validation_error")


def check_unrelated_work_pair(first, second):
    """Keep the control specific to a different target, not a different SOK."""
    a, b = first["snark_work"], second["snark_work"]
    if not a or not b:
        raise ValueError("unrelated-work control requires two work proofs")
    if first["submitter"] != second["submitter"]:
        raise ValueError("unrelated-work control requires equal submitters")
    for field in ("fee", "sok_message", "proof_sok_digest_hex", "expected_sok_digest_hex"):
        if a[field] != b[field]:
            raise ValueError(f"unrelated-work control requires equal {field}")
    for decoded, work in ((first, a), (second, b)):
        if (work["sok_matches"] is not True
                or work["proof_sok_digest_hex"] != work["expected_sok_digest_hex"]):
            raise ValueError("unrelated-work control requires matching SOK digests")
        if (work["target_matches_block"] is not True or
                work["statement"]["target"]["second_pass_ledger"] != decoded["staged_ledger_ledger_hash"]):
            raise ValueError("unrelated-work control requires each proof's block target")
    if first["staged_ledger_ledger_hash"] == second["staged_ledger_ledger_hash"]:
        raise ValueError("unrelated-work control requires different block ledger targets")


def prepare(root):
    cases = []
    for directory in sorted(root.iterdir()):
        if not directory.is_dir():
            continue
        request = json.loads((directory / "uptime-1.json").read_bytes())
        manifest = json.loads((directory / "manifest.json").read_text())
        cases.append({
            "case": directory.name,
            "submitted_at": datetime.datetime.fromtimestamp(
                manifest["received_at_unix"], datetime.timezone.utc).isoformat(),
            "block_hash": manifest["state_hash"],
            "raw_block": request["data"]["block"],
            "snark_work": request["data"].get("snark_work"),
            "submitter": request["submitter"],
        })
    if len(cases) < 2:
        raise ValueError("unrelated-work control requires at least two fixtures")
    pair = [json.loads((root / case["case"] / "decoded.json").read_text())
            for case in cases[:2]]
    for case, decoded in zip(cases, pair):
        if case["submitter"] != decoded["submitter"]:
            raise ValueError("fixture submitter differs from decoded metadata")
    check_unrelated_work_pair(*pair)
    controls = []
    for name in ("absent-work", "wrong-submitter", "truncated-work", "corrupt-proof", "unrelated-work"):
        case = copy.deepcopy(cases[0])
        case["case"] = "control-" + name
        if name == "absent-work":
            case["snark_work"] = None
        elif name == "wrong-submitter":
            # Public key from a separate unfunded throwaway capture attempt.
            case["submitter"] = "B62qra4RksF1rLx8gULmbxbpWxMKWhFrPgPbLoXaPGz3GJyGZQG6pE8"
        elif name == "truncated-work":
            case["snark_work"] = base64.b64encode(
                base64.b64decode(case["snark_work"])[:-64]).decode()
        elif name == "corrupt-proof":
            raw = bytearray(base64.b64decode(case["snark_work"]))
            decoded = json.loads((root / cases[0]["case"] / "decoded.json").read_text())
            raw[decoded["snark_work"]["ledger_proof_bytes"] - 32] ^= 1
            case["snark_work"] = base64.b64encode(raw).decode()
        else:
            # Same submitter/fee, independently valid work for another block.
            # delegation_verify does not check the target against this block.
            case["snark_work"] = cases[1]["snark_work"]
        controls.append(case)
    for case in cases + controls:
        # Upstream json_update only replaces fields already present in input.
        case.update({field: "NOT_VALIDATED" for field in RESULT_FIELDS})
    return cases + controls


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixtures", type=Path, default=Path(__file__).parent / "full-proof")
    parser.add_argument("--verifier", type=Path, required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    assert os.environ.get("MINA_PROFILE") == "devnet"
    assert os.environ.get("TMPDIR") and Path(os.environ["TMPDIR"]).is_dir()
    args.output.mkdir(parents=True, exist_ok=False)
    cases = prepare(args.fixtures)
    payload = (json.dumps(cases) + "\n").encode()
    (args.output / "input.json").write_bytes(payload)
    command = [str(args.verifier.resolve()), "stdin", "--signature-kind", "testnet",
               "--config-file", str(args.config.resolve())]
    started = time.time()
    with (args.output / "stdout.jsonl").open("wb") as stdout, (args.output / "stderr.log").open("wb") as stderr:
        process = subprocess.run(command, input=payload, stdout=stdout, stderr=stderr,
                                 timeout=1800, check=False)
    results = []
    for line in (args.output / "stdout.jsonl").read_text().splitlines():
        try:
            result = json.loads(line)
        except json.JSONDecodeError:
            continue
        if "case" in result:
            results.append({k: v for k, v in result.items() if k not in ("raw_block", "snark_work")})
    evidence = {
        "verifier_sha256": hashlib.sha256(args.verifier.read_bytes()).hexdigest(),
        "config_sha256": hashlib.sha256(args.config.read_bytes()).hexdigest(),
        "input_sha256": hashlib.sha256(payload).hexdigest(),
        "stdout_sha256": hashlib.sha256((args.output / "stdout.jsonl").read_bytes()).hexdigest(),
        "validation_enabled": True, "signature_kind": "testnet",
        "started_at_unix": started, "elapsed_seconds": time.time() - started,
        "exit_code": process.returncode, "results": results,
        "controls_are_derived_not_daemon_captures": True,
    }
    (args.output / "results.json").write_text(json.dumps(evidence, indent=2) + "\n")
    assert process.returncode == 0, "verifier process failed; inspect output"
    assert sorted(r["case"] for r in results) == sorted(c["case"] for c in cases)
    expected_rejections = {"control-wrong-submitter", "control-truncated-work", "control-corrupt-proof"}
    for result in results:
        assert all(result[k] != "NOT_VALIDATED" for k in RESULT_FIELDS)
        if result["case"] in expected_rejections:
            assert isinstance(result["validation_error"], str) and result["validation_error"]
            assert result["state_hash"] is None and result["height"] is None
        else:
            assert result["validation_error"] is None
            assert result["state_hash"] == result["block_hash"]
            assert isinstance(result["height"], int) and result["height"] > 0
        print(result["case"], result["validation_error"] or "ACCEPTED")


if __name__ == "__main__":
    main()
