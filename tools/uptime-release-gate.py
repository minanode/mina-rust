#!/usr/bin/env python3
"""Run a release binary for two uptime intervals, then verify every report.

Requires Linux user systemd and qualified delegation_verify and uptime_inspect.
The retained 4.0.3 bundle remains the baseline; 4.0.4 changes verifier output
handling. See OCAML-404-TOOL-QUALIFICATION before switching tool bundles.
Uptime is disk-only. Selected-network peers and reference artifacts need network access.
The output directory must be new. No existing key or node directory is used.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import time
import urllib.request
import urllib.parse


ROOT = Path(__file__).resolve().parents[1]
INTERVAL = 450
RESERVE = 55 * 1024**3


def run(*command, **kwargs):
    return subprocess.run(command, check=True, **kwargs)


def report_window(reports, first_seen, now, smoke=False):
    """Keep running for two complete intervals after the first saved report."""
    if smoke:
        return bool(reports)
    return first_seen is not None and now - first_seen >= 2 * INTERVAL and len(reports) >= 3


def check_status(status):
    uptime = status["uptime"]
    if uptime is None or uptime["attempts"] != 0:
        raise RuntimeError("expected an enabled disk-only reporter with zero HTTP attempts")
    if status["node_status"] is not None or status["node_error"] is not None:
        raise RuntimeError("unexpected additional reporter")


def verify_reports(reports, inspect, verifier, output, env, log, network="devnet", binary=None):
    # -E prevents PYTHONOPTIMIZE from disabling the reference harness checks.
    run(sys.executable, "-E", "-B", str(ROOT / "tools/uptime-fixtures/verify_reports.py"),
        "--reports", str(reports), "--inspect", str(inspect),
        "--verifier", str(verifier), "--network", network,
        "--config", str(ROOT / f"tools/uptime-fixtures/full-proof/{network}-config.json"),
        "--output", str(output), "--check-http-signatures", "--allow-zero-sok-zkapp",
        *(["--work-verifier-binary", str(binary)] if binary is not None else []),
        env=env, stdout=log, stderr=log)


def genesis_height(network):
    config = json.loads((ROOT / f"tools/uptime-fixtures/full-proof/{network}-config.json").read_text())
    return config["proof"]["fork"]["blockchain_length"] + 1


def check_report_heights(decoded, genesis_height):
    if not decoded or any(row["height"] <= genesis_height for row in decoded):
        raise RuntimeError("a report contains a genesis or pre-fork placeholder")


def restored_frontier(persist):
    # StateGet's JSONPath response is a list of matches.
    if isinstance(persist, list):
        persist = persist[0] if len(persist) == 1 else {}
    if not isinstance(persist, dict):
        return False
    ready = persist.get("Ready", {})
    restore = ready.get("restore", {})
    if not isinstance(restore, dict):
        return False
    outcome = restore.get("Done", {}).get("outcome", {})
    return isinstance(outcome, dict) and "Restored" in outcome




def sample_frontier(opener, http_port, read_restore):
    # These state-machine RPCs can be delayed while a block is applied. They
    # are evidence samples, separate from the mandatory disk-only status check.
    sample = {"frontier": None, "persist": None, "sample_errors": {}}
    queries = [("frontier", "/status", 5)]
    if read_restore:
        query = urllib.parse.urlencode({"filter": "$.transition_frontier.persist"})
        queries.append(("persist", f"/state?{query}", 30))
    for name, route, timeout in queries:
        try:
            with opener.open(f"http://127.0.0.1:{http_port}{route}", timeout=timeout) as response:
                value = json.load(response)
            sample[name] = value["transition_frontier"] if name == "frontier" else value
        except (OSError, TimeoutError) as error:
            sample["sample_errors"][name] = str(error)
    return sample

def start_node(unit, output, password_file, launcher, timeout):
    run("systemd-run", "--user", "--quiet", "--unit", unit,
        "--property=Type=exec", "--property=KillMode=control-group",
        "--property=TimeoutStopSec=30", f"--property=RuntimeMaxSec={timeout}",
        "--property=EnvironmentFile=" + str(password_file),
        "--property=StandardOutput=append:" + str(output / "node.log"),
        "--property=StandardError=append:" + str(output / "node.log"), str(launcher))


def stop_node(unit, reports):
    # A report can finish during shutdown. Freeze the complete file set only
    # after the node stops, so none can be attributed to the next process.
    run("systemctl", "--user", "stop", unit)
    return sorted(reports.glob("*.json"))

def port_number(value):
    port = int(value)
    if not 1 <= port <= 65535:
        raise argparse.ArgumentTypeError("port must be between 1 and 65535")
    return port


def argument_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--commit", required=True, help="expected full build commit")
    parser.add_argument("--verifier", type=Path, required=True)
    parser.add_argument("--inspect", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--network", choices=("mainnet", "devnet"), default="devnet")
    parser.add_argument("--http-port", type=port_number, default=4330)
    parser.add_argument("--libp2p-port", type=port_number, default=9450)
    parser.add_argument("--timeout", type=int, default=2400, help="total node runtime limit in seconds")
    parser.add_argument("--restart", action="store_true",
                        help="repeat the report window after restarting from the same frontier store")
    parser.add_argument("--smoke", action="store_true",
                        help="stop after one report per phase; not a full release qualification")
    return parser


def main():
    parser = argument_parser()
    args = parser.parse_args()
    if args.timeout <= 0 or (not args.smoke and args.timeout <= 2 * INTERVAL):
        parser.error("timeout must allow bootstrap and two full intervals")
    for name in ("binary", "verifier", "inspect"):
        path = getattr(args, name).resolve(strict=True)
        if not os.access(path, os.X_OK):
            parser.error(f"{name} must be executable")
        setattr(args, name, path)
    for port in (args.http_port, args.libp2p_port):
        with socket.socket() as probe:
            probe.bind(("0.0.0.0", port))
    output = args.output.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    (output / "tmp").mkdir(mode=0o700)
    (output / "reports").mkdir(mode=0o700)
    unit = "mina-uptime-gate-" + secrets.token_hex(8)
    key = output / "submitter"
    password_file = output / "password.env"
    password = secrets.token_hex(32)
    # Do not inherit MINA_* options, production keys, other reporters or proxies.
    node_env = {"PATH": os.defpath, "HOME": os.environ["HOME"],
                "UPTIME_PRIVKEY_PASS": password}
    password_file.write_text("UPTIME_PRIVKEY_PASS=" + password + "\n")
    password_file.chmod(0o600)
    started_unit = False
    started = time.monotonic()
    first_seen = None
    phase = 0
    phase_files = set()
    restore_seen = False
    minimum_free = shutil.disk_usage(output).free
    result = {"passed": False, "unit": unit, "interval_seconds": INTERVAL,
              "network": args.network, "phases": [],
              "mode": "smoke" if args.smoke else "release", "exceptions": []}

    def interrupted(signum, frame):
        raise RuntimeError(f"gate interrupted by signal {signum}")

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    try:
        if minimum_free < RESERVE:
            raise RuntimeError("less than 55 GiB free")
        provenance = run(str(ROOT / "tools/regression/check-build-env.sh"),
                         str(args.binary), args.commit, capture_output=True, text=True)
        result["build"] = json.loads(provenance.stdout)
        with (output / "keygen.log").open("w") as log:
            run(str(args.binary), "wallet", "generate", "--output", str(key),
                env={"PATH": os.defpath, "MINA_PRIVKEY_PASS": password}, stdout=log, stderr=log)
        # execve clears systemd manager defaults. The password remains in the
        # environment; even a short-lived env NAME=value argv would expose it.
        launcher = output / "start-node.py"
        command = [str(args.binary), "node", "--network", args.network, "--work-dir", str(output / "node"),
                   "--port", str(args.http_port), "--http-addr", "127.0.0.1",
                   "--libp2p-port", str(args.libp2p_port), "--uptime-url", "http://127.0.0.1:8850/v1/submit",
                   "--uptime-submitter-key", str(key), "--uptime-dry-run-dir", str(output / "reports")]
        if args.network == "mainnet":
            command = [str(ROOT / "tools/regression/lock.sh"), "node", "--", *command]
        launcher.write_text("#!/usr/bin/python3\nimport os\ncommand = " + repr(command) +
                            "\nos.execve(command[0], command, {\"PATH\": os.defpath, \"HOME\": " +
                            repr(node_env["HOME"]) + ', "UPTIME_PRIVKEY_PASS": os.environ["UPTIME_PRIVKEY_PASS"]})\n')
        launcher.chmod(0o700)
        started_unit = True  # also clean up if systemd accepts but the client fails
        start_node(unit, output, password_file, launcher, args.timeout)
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        while time.monotonic() - started < args.timeout:
            active = subprocess.run(["systemctl", "--user", "is-active", "--quiet", unit]).returncode
            if active:
                raise RuntimeError("transient node stopped before the gate completed")
            free = shutil.disk_usage(output).free
            minimum_free = min(minimum_free, free)
            if free < RESERVE:
                raise RuntimeError("free space fell below 55 GiB")
            reports = list((output / "reports").glob("*.json"))
            current_reports = [path for path in reports if path.name not in phase_files]
            if current_reports and first_seen is None:
                first_seen = time.monotonic()
            try:
                with opener.open(f"http://127.0.0.1:{args.http_port}/reporting/status", timeout=5) as response:
                    status = json.load(response)
                check_status(status)
                (output / "status.json").write_text(json.dumps(status, indent=2) + "\n")
            except (OSError, TimeoutError):
                if current_reports:
                    raise
                status = None
            # The restore outcome persists after completion. Read it only
            # after the restarted node has saved work, then stop polling the
            # expensive state endpoint once completion has been established.
            sample = sample_frontier(opener, args.http_port,
                                     phase == 1 and bool(current_reports) and not restore_seen)
            if phase == 1:
                restore_seen = restore_seen or restored_frontier(sample["persist"])
            with (output / "samples.jsonl").open("a") as samples:
                samples.write(json.dumps({"time": time.time(), "phase": phase,
                    "reports": len(current_reports), "reporting": status, **sample}) + "\n")
            if report_window(current_reports, first_seen, time.monotonic(), args.smoke):
                reports = stop_node(unit, output / "reports")
                started_unit = False
                current_reports = [path for path in reports if path.name not in phase_files]
                result["phases"].append({"phase": phase, "reports": len(current_reports),
                    "seconds_after_first_report": time.monotonic() - first_seen})
                if args.restart and phase == 0:
                    # Start a new owned unit: inactive transient units may
                    # unload. Keep the same key, reports and frontier store.
                    phase_files = {path.name for path in reports}
                    result["restart_at_unix"] = time.time()
                    unit += "-restart"
                    result["restart_unit"] = unit
                    started_unit = True
                    start_node(unit, output, password_file, launcher, args.timeout)
                    phase = 1
                    first_seen = None
                    continue
                if args.restart and not restore_seen:
                    raise RuntimeError("restart did not restore the frontier")
                break
            time.sleep(10)
        else:
            raise RuntimeError("timed out before three reports and two complete intervals")
        result["node_runtime_seconds"] = time.monotonic() - started
        result["seconds_after_first_report"] = time.monotonic() - first_seen
        # The node is stopped, so this verifies the final, complete file set.
        reports = sorted((output / "reports").glob("*.json"))
        result["reports"] = [{"file": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                             for path in reports]
        if not any(json.loads(path.read_bytes())["data"].get("snark_work") for path in reports):
            raise RuntimeError("no report exercised SNARK work")
        verify_env = dict(os.environ, MINA_PROFILE=args.network, TMPDIR=str(output / "tmp"))
        with (output / "verification.log").open("w") as log:
            verify_reports(output / "reports", args.inspect, args.verifier,
                           output / "verification", verify_env, log, args.network, args.binary)
        decoded = json.loads((output / "verification/decoded.json").read_text())
        result["exceptions"] = json.loads((output / "verification/results.json").read_text())["exceptions"]
        check_report_heights(decoded, genesis_height(args.network))
        result["no_genesis_reports"] = True
        result["frontier_restored"] = restore_seen
        result["passed"] = True
    except Exception as error:
        result["error"] = str(error)
        raise
    finally:
        if started_unit:
            stopped = subprocess.run(["systemctl", "--user", "stop", unit])
            if stopped.returncode:
                result["passed"] = False
                result["cleanup_error"] = "could not stop transient unit " + unit
        for private in (key, password_file):
            private.unlink(missing_ok=True)
        result["minimum_free_bytes"] = minimum_free
        (output / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(f"PASS ({result['mode']}): {len(reports)} reports; "
          f"{len(result['exceptions'])} evidenced OCaml zero-SOK exceptions; "
          "all other reports fully verified")


if __name__ == "__main__":
    main()
