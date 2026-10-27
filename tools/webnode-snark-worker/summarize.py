#!/usr/bin/env python3
"""P5.3: summarise a measurement (measure.mjs) or gate (gate-browser.mjs)
result.json: prover setup phases and memory, each proof's time, peaks of
the node's and the prover's linear memory and of the browser's RSS and PSS,
and the host's load. Reads only the given file; prints JSON.

    python3 -I tools/webnode-snark-worker/summarize.py RESULT.json
"""

import json
import statistics
import sys

MIB = 1024 * 1024


def stats(values):
    if not values:
        return None
    return {"count": len(values), "min_s": round(min(values) / 1000, 2),
            "median_s": round(statistics.median(values) / 1000, 2),
            "max_s": round(max(values) / 1000, 2)}


def measurement(result):
    runs = []
    for run in result["runs"]:
        setup = run.get("setup", {})
        phases = {key[:-3]: round(value / 1000, 2) for key, value in setup.items()
                  if key.endswith("_ms") and not key.endswith("linear_bytes_ms")}
        linear = {key[: -len("_linear_bytes")]: round(value / MIB, 1)
                  for key, value in setup.items() if key.endswith("_linear_bytes")}
        runs.append({
            "config": run["config"],
            "setup_s": round(run.get("setupMs", 0) / 1000, 1),
            "setup_phases_s": phases,
            "linear_after_phase_mib": linear,
            "proofs": [{"what": p["what"], "s": round(p["ms"] / 1000, 2),
                        "linear_after_mib": round((p.get("linearBytesAfter") or 0) / MIB, 1),
                        "request_bytes": p["requestBytes"], "response_bytes": p["responseBytes"]}
                       for p in run["proofs"]],
            "peak_linear_mib": round((run.get("peakLinearBytes") or 0) / MIB, 1),
            "failures": run.get("failures"),
        })
    return {"runs": runs}


def gate(result):
    events = result.get("events", [])
    ends = [e for e in events if e.get("type") == "work-end"]
    hosts = [e for e in events if e.get("type") in ("host-started", "host-ready", "host-failed", "host-retired")]
    peak_prover = max((result.get("peak", {}).get("prover") or {"0": 0}).values())
    return {
        "bundles_proved": len(ends),
        "bundle_round_trip": stats([e["ms"] for e in ends]),
        "first_bundle_at": min((e["at"] for e in ends), default=None),
        "hosts": [{k: e.get(k) for k in ("type", "generation", "at", "reason")} for e in hosts],
        "peak_node_linear_mib": round(result.get("peak", {}).get("node", 0) / MIB, 1),
        "peak_prover_linear_mib": round(peak_prover / MIB, 1),
        "last_status": (result.get("status") or [None])[-1],
    }


def main():
    result = json.load(open(sys.argv[1]))
    summary = measurement(result) if "runs" in result else gate(result)
    memory = result.get("browserMemory", {})
    peak = memory.get("peak", {})
    loads = [s["load"] for s in memory.get("samples", []) if "load" in s]
    summary["browser_peak_rss_mib"] = round(peak.get("rssKiB", 0) / 1024, 1)
    summary["browser_peak_pss_mib"] = round(peak.get("pssKiB", 0) / 1024, 1)
    summary["load_1min"] = ({"min": min(loads), "median": statistics.median(loads), "max": max(loads)}
                            if loads else None)
    summary["machine"] = result.get("machine")
    summary["artifacts"] = result.get("artifacts")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
