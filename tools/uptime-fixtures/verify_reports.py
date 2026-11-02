#!/usr/bin/env python3
"""Check saved reports with OCaml's full delegation verifier.

No uptime requests are sent. --check-http-signatures additionally uses OCaml's
legacy Schnorr verifier: delegation_verify itself checks the blockchain proof,
work proof and fee/submitter SOK, not the outer signature.
The reference may fetch public verification keys and genesis data at startup.
Use a private TMPDIR and MINA_PROFILE matching --network.
"""

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

from verify_full_proof import RESULT_FIELDS


SOK_REJECTION = "proof's sok message digest does not match the sok message"


def terminal_zkapp_statement(statement):
    """Recognize an active zkApp execution ending in canonical empty state.

    Ordinary jobs have empty local state on both sides. A single-segment zkApp
    can be ambiguous too: fail closed rather than trust a node's job-kind flag.
    The empty-state constants are independently decoded OCaml capture values.
    """
    capture = Path(__file__).parent / "full-proof/devnet-581348-coinbase/decoded.json"
    empty = json.loads(capture.read_text())["snark_work"]["statement"]["target"]["local_state"]
    try:
        source = statement["source"]["local_state"]
        target = statement["target"]["local_state"]
        return (target == empty and set(source) == set(empty)
                and int(source["account_update_index"]) > 0
                and int(source["transaction_commitment"], 16) > 0
                and int(source["full_transaction_commitment"], 16) > 0)
    except (KeyError, TypeError, ValueError):
        return False


def zero_sok_zkapp_exception(inspection):
    """Classify only decoded statement evidence, never a supplied job label."""
    work = inspection.get("snark_work")
    if not work or work["sok_matches"]:
        return None
    if (not work["sok_is_default"] or work["proof_sok_digest_hex"] != "00" * 32
            or not terminal_zkapp_statement(work["statement"])):
        raise ValueError("SOK mismatch is not an evidenced terminal zkApp zero digest")
    return {"kind": "ocaml_terminal_zkapp_zero_sok",
            "classification": "decoded_work_statement",
            "source_account_update_index": work["statement"]["source"]["local_state"]["account_update_index"],
            "terminal_local_state": "canonical_empty",
            "statement_sha256": hashlib.sha256(json.dumps(work["statement"], sort_keys=True).encode()).hexdigest()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reports", type=Path, required=True)
    parser.add_argument("--inspect", type=Path, required=True)
    parser.add_argument("--verifier", type=Path, required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--network", choices=("mainnet", "devnet"), default="devnet")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--expect-sok-rejection", action="store_true")
    mode.add_argument("--allow-zero-sok-zkapp", action="store_true")
    parser.add_argument("--check-http-signatures", action="store_true")
    parser.add_argument("--work-verifier-binary", type=Path,
                        help="mina binary with internal verify-uptime-work; required for a zero-SOK exception")
    args = parser.parse_args()
    if args.allow_zero_sok_zkapp and not args.check_http_signatures:
        parser.error("the zkApp exception requires --check-http-signatures")
    assert os.environ.get("MINA_PROFILE") == args.network, "MINA_PROFILE/network mismatch"
    signature_kind = "mainnet" if args.network == "mainnet" else "testnet"
    assert os.environ.get("TMPDIR") and Path(os.environ["TMPDIR"]).is_dir()
    args.output.mkdir(parents=True, exist_ok=False)
    inputs, decoded, candidates, expected_blocks = [], [], {}, {}
    for path in sorted(args.reports.glob("*.json")):
        raw = path.read_bytes()
        request = json.loads(raw)
        assert request["version"] == 1
        inspect_command = [str(args.inspect.resolve()), str(path.resolve())]
        # Preserve the original decoder interface for existing devnet tools.
        # Mainnet requires the decoder with explicit signature-kind support.
        if args.network == "mainnet":
            inspect_command.extend(("--signature-kind", signature_kind))
        inspection = json.loads(subprocess.check_output(inspect_command, timeout=120))
        assert inspection["block_roundtrip_exact"]
        if args.check_http_signatures:
            assert inspection["http_signature_valid"]
        work = inspection["snark_work"]
        exception = None
        if work is not None:
            assert work["roundtrip_exact"]
            if not args.expect_sok_rejection:
                assert work["target_matches_block"]
                if args.allow_zero_sok_zkapp:
                    exception = zero_sok_zkapp_exception(inspection)
                else:
                    assert work["sok_matches"]
        decoded.append({"case": path.name, "request_sha256": hashlib.sha256(raw).hexdigest(),
                        **inspection})
        inputs.append({"case": path.name, "submitted_at": request["data"]["created_at"],
                       "block_hash": inspection["state_hash"],
                       "raw_block": request["data"]["block"],
                       "snark_work": request["data"].get("snark_work"),
                       "submitter": request["submitter"],
                       **{field: "NOT_VALIDATED" for field in RESULT_FIELDS}})
        expected_blocks[path.name] = (inspection["state_hash"], inspection["height"])
        if exception is not None:
            if args.work_verifier_binary is None:
                raise ValueError("zero-SOK exception requires independent work-proof verification")
            raw_work = base64.b64decode(request["data"]["snark_work"], validate=True)
            proof_path = args.output / f"zero-sok-work-{len(candidates)}.bin"
            proof_path.write_bytes(raw_work)
            checked = subprocess.check_output(
                [str(args.work_verifier_binary.resolve()), "--network", args.network,
                 "internal", "verify-uptime-work", str(proof_path.resolve())], timeout=300)
            if checked.strip() != b"uptime work proof verified":
                raise ValueError("unexpected independent work-proof verification receipt")
            exception.update(work_proof_verified=True,
                             work_sha256=hashlib.sha256(raw_work).hexdigest(),
                             work_verifier_sha256=hashlib.sha256(args.work_verifier_binary.read_bytes()).hexdigest())
            # Keep the original full-verifier input and require its exact SOK
            # error. OCaml validates the block before SOK, but that error
            # clears its result fields. Obtain an explicit block receipt too.
            block_case = "zero-sok-block/" + path.name
            inputs.append({**inputs[-1], "case": block_case, "snark_work": None})
            expected_blocks[block_case] = expected_blocks[path.name]
            candidates[path.name] = {**exception, "case": path.name,
                                     "request_sha256": hashlib.sha256(raw).hexdigest(),
                                     "block_validation_case": block_case}
    assert inputs, "no reports"
    (args.output / "decoded.json").write_text(json.dumps(decoded, indent=2) + "\n")
    payload = (json.dumps(inputs) + "\n").encode()
    (args.output / "input.json").write_bytes(payload)
    started = time.time()
    with (args.output / "stdout.jsonl").open("wb") as stdout, (args.output / "stderr.log").open("wb") as stderr:
        process = subprocess.run([str(args.verifier.resolve()), "stdin", "--signature-kind", signature_kind,
                                  "--config-file", str(args.config.resolve())], input=payload,
                                 stdout=stdout, stderr=stderr, timeout=1800, check=False)
    results = []
    for line in (args.output / "stdout.jsonl").read_text().splitlines():
        try:
            result = json.loads(line)
        except json.JSONDecodeError:
            continue
        if "case" in result:
            results.append({key: value for key, value in result.items()
                            if key not in ("raw_block", "snark_work")})
    evidence = {"validation_enabled": True, "network": args.network, "signature_kind": signature_kind,
                "http_signatures_checked": args.check_http_signatures, "exit_code": process.returncode,
                "started_at_unix": started, "elapsed_seconds": time.time() - started,
                "verifier_sha256": hashlib.sha256(args.verifier.read_bytes()).hexdigest(),
                "config_sha256": hashlib.sha256(args.config.read_bytes()).hexdigest(),
                "input_sha256": hashlib.sha256(payload).hexdigest(), "results": results,
                "exceptions": []}
    (args.output / "results.json").write_text(json.dumps(evidence, indent=2) + "\n")
    assert process.returncode == 0
    assert sorted(r["case"] for r in results) == sorted(r["case"] for r in inputs)
    for result in results:
        assert all(result[key] != "NOT_VALIDATED" for key in RESULT_FIELDS)
        if args.expect_sok_rejection or result["case"] in candidates:
            assert result["validation_error"] == SOK_REJECTION
            assert result["state_hash"] is None
        else:
            assert result["validation_error"] is None
            assert result["verified"] is True
            expected_hash, expected_height = expected_blocks[result["case"]]
            assert result["state_hash"] == result["block_hash"] == expected_hash
            assert result["height"] == expected_height
            assert isinstance(result["height"], int) and result["height"] > 0
        print(result["case"], result["validation_error"] or "ACCEPTED")
    # All original and derived rows have passed their checks. Preserve the
    # reference rejection; never label a zero-SOK work proof fully verified.
    evidence["exceptions"] = [dict(candidate, validation_error=SOK_REJECTION,
                                   block_verified=True, http_signature_verified=True,
                                   work_roundtrip_exact=True, target_matches_block=True,
                                   work_and_sok_fully_verified=False)
                              for candidate in candidates.values()]
    (args.output / "results.json").write_text(json.dumps(evidence, indent=2) + "\n")


if __name__ == "__main__":
    main()
