#!/usr/bin/env python3
"""Cross-prove pinned jobs, check all four verifier cells, stop on disagreement."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import queue
import threading
import time


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def execute(command, prefix, environment, timeout):
    started = time.monotonic()
    timing = prefix.with_suffix(".time")
    with prefix.with_suffix(".log").open("wb") as log:
        process = subprocess.Popen(["/usr/bin/time", "-f", "%e %M", "-o", str(timing), *command],
                                   stdout=log, stderr=subprocess.STDOUT, env=environment, start_new_session=True)
        timed_out = False
        try:
            returncode = process.wait(timeout=timeout)
        except (subprocess.TimeoutExpired, KeyboardInterrupt):
            timed_out = True
            os.killpg(process.pid, signal.SIGTERM)
            try:
                returncode = process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                returncode = process.wait()
    result = {"exit_code": returncode, "timeout": timed_out,
              "wall_seconds": time.monotonic() - started, "peak_rss_kib": None,
              "host_load_before_next_step": list(os.getloadavg())}
    if timing.exists():
        try:
            wall, rss = timing.read_text().splitlines()[-1].split()
            result.update(time_wall_seconds=float(wall), peak_rss_kib=int(rss))
        except (ValueError, IndexError):
            pass
    return result


def verdict(metrics, receipt):
    if metrics["timeout"] or metrics["exit_code"] not in (0, 1) or not receipt.exists():
        return "unavailable"
    try:
        data = json.loads(receipt.read_text())
        if data.get("verification_panicked", False):
            return "unavailable"
        accepted = data["accepted"]
        if type(accepted) is not bool or metrics["exit_code"] != (0 if accepted else 1):
            return "unavailable"
    except (OSError, ValueError, KeyError):
        return "unavailable"
    return "accept" if accepted else "reject"


def select_cases(manifest, case_ids, subset):
    jobs = manifest["jobs"]
    job_ids = [case["id"] for case in jobs]
    smoke_ids = manifest["smoke_cases"]
    for label, ids in (("job", job_ids), ("smoke", smoke_ids), ("requested", case_ids or [])):
        if any(not isinstance(value, str) or not value for value in ids):
            raise ValueError(f"invalid {label} ID")
        if len(ids) != len(set(ids)):
            raise ValueError(f"duplicate {label} IDs")
    if case_ids:
        requested = case_ids
    elif subset == "smoke":
        requested = smoke_ids
    else:
        target = manifest["target_jobs"]
        if type(target) is not int or target <= 0 or len(jobs) != target:
            raise ValueError("full corpus count does not match target_jobs")
        requested = job_ids
    missing = set(requested) - set(job_ids)
    if missing:
        raise ValueError("missing requested case IDs: " + ", ".join(sorted(missing)))
    if not requested:
        raise ValueError("empty selection")
    return [case for case in jobs if case["id"] in requested]


def complete_results(cases, expected_ids, negative_controls):
    if not expected_ids or [case["id"] for case in cases] != expected_ids:
        return False
    implementations = {"rust", "ocaml"}
    controls = {"wrong_fee", "wrong_network"} if negative_controls else set()
    for case in cases:
        if set(case["provers"]) != implementations or set(case["matrix"]) != implementations:
            return False
        if any(p["exit_code"] != 0 or p["timeout"] or not p.get("proof_sha256")
               for p in case["provers"].values()):
            return False
        if any(set(cells) != implementations or any(c["verdict"] != "accept" for c in cells.values())
               for cells in case["matrix"].values()):
            return False
        if set(case["negative_controls"]) != controls:
            return False
        if any(set(cells) != implementations or any(c["verdict"] != "reject" for c in cells.values())
               for cells in case["negative_controls"].values()):
            return False
    return True


class Server:
    """One network per process; stdout receipts are separate from diagnostic logs."""
    shutdown_timeout = 10

    def __init__(self, binary, network, environment, directory):
        self.log = (directory / (binary.name + "-" + network + ".log")).open("w")
        self.process = subprocess.Popen([str(binary), network, "serve"], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                        text=True, bufsize=1, env={**environment, "MINA_PROFILE": network}, start_new_session=True)
        self.receipts = queue.Queue()
        self.requests = 0
        self.shutdown_result = None

        def read():
            for line in self.process.stdout:
                self.log.write(line)
                self.log.flush()
                if line.startswith("CORPUS_RESULT "):
                    self.receipts.put(json.loads(line[len("CORPUS_RESULT "):]))
            self.receipts.put({"exit_code": 2, "error": "server terminated"})

        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()

    def request(self, command, timeout):
        started = time.monotonic()
        cold = self.requests == 0
        self.requests += 1
        peak = 0
        highwater = 0
        try:
            self.process.stdin.write(json.dumps(command[1:]) + "\n")
            self.process.stdin.flush()
        except BrokenPipeError:
            return {"exit_code": 2, "timeout": False, "error": "worker pipe closed",
                    "wall_seconds": time.monotonic() - started, "peak_rss_kib": peak,
                    "cold_process": cold}
        while True:
            try:
                status = Path(f"/proc/{self.process.pid}/status").read_text()
                fields = {line.split(":")[0]: line.split(":", 1)[1] for line in status.splitlines()}
                peak = max(peak, int(fields.get("VmRSS", "0 kB").split()[0]))
                highwater = max(highwater, int(fields.get("VmHWM", "0 kB").split()[0]))
            except (FileNotFoundError, ProcessLookupError):
                pass
            if time.monotonic() - started > timeout:
                self.close()
                return {"exit_code": 2, "timeout": True, "wall_seconds": time.monotonic() - started,
                        "peak_rss_kib": peak, "cold_process": cold}
            try:
                receipt = self.receipts.get(timeout=0.02)
                return {**receipt, "timeout": False, "wall_seconds": time.monotonic() - started,
                        "peak_rss_kib": peak, "process_highwater_rss_kib": highwater,
                        "cold_process": cold, "host_load_before_next_step": list(os.getloadavg())}
            except queue.Empty:
                continue

    def close(self):
        if self.shutdown_result is not None:
            return self.shutdown_result
        forced = False
        try:
            self.process.stdin.close()
        except BrokenPipeError:
            pass
        if self.process.poll() is None:
            try:
                self.process.wait(timeout=self.shutdown_timeout)
            except subprocess.TimeoutExpired:
                forced = True
                try:
                    os.killpg(self.process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    self.process.wait(timeout=self.shutdown_timeout)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(self.process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    self.process.wait()
        self.reader.join(timeout=5)
        self.process.stdout.close()
        self.log.close()
        self.shutdown_result = {"exit_code": self.process.returncode, "forced_termination": forced}
        return self.shutdown_result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=Path(__file__).with_name("manifest.json"))
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--rust", type=Path, required=True)
    parser.add_argument("--ocaml", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--case", action="append")
    parser.add_argument("--subset", choices=("smoke", "all"), default="smoke")
    parser.add_argument("--threads", type=int, default=8)
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--negative-controls", action="store_true", help="check wrong SOK fee and wrong network for each Rust proof")
    parser.add_argument("--process-mode", choices=("warm", "cold"), default="warm")
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text())
    try:
        selected = select_cases(manifest, args.case, args.subset)
    except ValueError as error:
        parser.error(str(error))
    # Validate the complete selection before doing any expensive work.
    seen = set()
    for case in selected:
        identity = (case["network"], case["statement_binprot_sha256"])
        if identity in seen:
            raise SystemExit("duplicate job statement in selection")
        seen.add(identity)
        path = (args.corpus / case["input"]["path"]).resolve()
        if not path.is_relative_to(args.corpus.resolve()) or sha256(path) != case["input"]["sha256"]:
            raise SystemExit("job input hash/path mismatch: " + case["id"])
    args.output.mkdir(parents=True, exist_ok=False)
    out = args.output.resolve()
    tmp = out / "tmp"
    tmp.mkdir(mode=0o700)
    message = out / "message.json"
    message.write_text(json.dumps(manifest["message"]) + "\n")
    env = os.environ.copy()
    env.update(RAYON_NUM_THREADS=str(args.threads), OMP_NUM_THREADS=str(args.threads), TMPDIR=str(tmp))
    binaries = {"rust": args.rust.resolve(), "ocaml": args.ocaml.resolve()}
    servers = {}

    def close_server(key):
        result = servers.pop(key).close()
        worker = next(name for name, path in binaries.items() if str(path) == key[0])
        report["worker_exits"].append({"worker": worker, "network": key[1], **result})
        return result

    def invoke(command, prefix, environment, timeout):
        if args.process_mode == "cold":
            return execute(command, prefix, environment, timeout)
        key = (command[0], command[1])
        if key not in servers:
            servers[key] = Server(Path(command[0]), command[1], environment, out)
        result = servers[key].request(command, timeout)
        if result.get("timeout") or servers[key].process.poll() is not None:
            status = close_server(key)
            if status["exit_code"] != 0 or status["forced_termination"]:
                result.update(exit_code=2, worker_exit=status)
        return result

    report = {"manifest_sha256": sha256(args.manifest), "binary_sha256": {key: sha256(path) for key, path in binaries.items()},
              "threads": args.threads, "timeout_seconds_per_process": args.timeout,
              "process_mode": args.process_mode,
              "measurement": "warm: process RSS sampled every 20 ms per operation, plus process lifetime highwater; cold: GNU time maximum RSS. Adapter proof time recorded separately. One sequential worker per network and implementation; verification may initialize keys before the first OCaml proof.",
              "expected_cases": [case["id"] for case in selected], "worker_exits": [],
              "cases": [], "divergence": None, "passed": False}
    start = time.monotonic()

    def save():
        report["gate_seconds"] = time.monotonic() - start
        (out / "results.json").write_text(json.dumps(report, indent=2) + "\n")

    save()
    try:
        for case in selected:
            if shutil.disk_usage(out).free < 56_000_000_000:
                save()
                raise SystemExit("gate stopped: less than 56 GB disk free")
            directory = out / case["id"]
            directory.mkdir()
            job = (args.corpus / case["input"]["path"]).resolve()
            network = case["network"]
            env["MINA_PROFILE"] = network
            record = {"id": case["id"], "network": network, "coverage": case["coverage"], "provers": {}, "matrix": {}, "negative_controls": {}}
            report["cases"].append(record)
            for prover, binary in binaries.items():
                proof = directory / (prover + ".bin")
                command = [str(binary), network, "prove", str(job), str(proof), str(message)]
                if prover == "ocaml":
                    command.append("unused")
                metrics = invoke(command, directory / (prover + "-prove"), env, args.timeout)
                record["provers"][prover] = metrics
                receipt = Path(str(proof) + ".json")
                if metrics["exit_code"] == 0 and not metrics["timeout"] and proof.exists() and receipt.exists():
                    metrics.update(json.loads(receipt.read_text()), proof_sha256=sha256(proof))
                    cells = {}
                    for verifier, verifier_binary in binaries.items():
                        prefix = directory / (prover + "-to-" + verifier)
                        result_path = prefix.with_suffix(".json")
                        checked = invoke([str(verifier_binary), network, "verify", str(job), str(result_path), str(message), str(proof)], prefix, env, args.timeout)
                        cells[verifier] = {**checked, "verdict": verdict(checked, result_path)}
                    record["matrix"][prover] = cells
                    outcomes = {cell["verdict"] for cell in cells.values()}
                    if outcomes == {"accept", "reject"}:
                        report["divergence"] = {"case": case["id"], "prover": prover, "proof_sha256": sha256(proof)}
                        (out / "divergence-case.json").write_text(json.dumps(case, indent=2) + "\n")
                        save()
                        raise SystemExit("VERIFIER DIVERGENCE: preserved single-job reproducer; stopping")
                else:
                    record["matrix"][prover] = {v: {"verdict": "unavailable"} for v in binaries}
                save()
            if args.negative_controls and record["matrix"]["rust"]["rust"]["verdict"] == "accept":
                wrong_message = directory / "wrong-message.json"
                wrong_message.write_text(json.dumps({**manifest["message"], "fee": "0.002"}) + "\n")
                proof = directory / "rust.bin"
                for control in ("wrong_fee", "wrong_network"):
                    cells = {}
                    verify_network = ("devnet" if network == "mainnet" else "mainnet") if control == "wrong_network" else network
                    env["MINA_PROFILE"] = verify_network
                    for verifier, binary in binaries.items():
                        prefix = directory / (control + "-" + verifier)
                        receipt = prefix.with_suffix(".json")
                        metrics = invoke([str(binary), verify_network, "verify", str(job), str(receipt),
                                           str(wrong_message if control == "wrong_fee" else message), str(proof)], prefix, env, args.timeout)
                        cells[verifier] = {**metrics, "verdict": verdict(metrics, receipt)}
                    record["negative_controls"][control] = cells
                    if {cell["verdict"] for cell in cells.values()} == {"accept", "reject"}:
                        report["divergence"] = {"case": case["id"], "control": control, "prover": "rust"}
                        save()
                        raise SystemExit("VERIFIER DIVERGENCE on negative control; stopping")
                    save()
            print(case["id"], {p: {v: c["verdict"] for v, c in cells.items()} for p, cells in record["matrix"].items()}, flush=True)
    finally:
        # A receipt is not an OS exit status. Reap every worker before success,
        # including workers that failed earlier and were replaced by invoke().
        for key in list(servers):
            close_server(key)
        save()
    report["passed"] = complete_results(report["cases"], report["expected_cases"], args.negative_controls)
    report["passed"] &= all(worker["exit_code"] == 0 and not worker["forced_termination"]
                            for worker in report["worker_exits"])
    save()
    print("gate_seconds:", round(report["gate_seconds"], 2), "passed:", report["passed"])
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
