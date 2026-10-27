#!/usr/bin/env python3
"""Break a native bootstrap into phases from its log and, if present, the
sampler's output.

Usage: analyse.py LOG [SAMPLES_DIR] [--start ISO8601] [--until ISO8601]

Reads both log formats the node writes (`kind="..."` in log files,
`kind: "..."` on stdout). With patches/rpc-logging.patch applied to the node
it also counts requests per peer and kind, with latencies. The block
applications, disconnections and later sync targets are logged at DEBUG since
O1: run-node.sh runs the node with `--verbosity debug`. The `P2pChannelsRpc*`
and `P2pNetworkScheduler*` actions it counts are TRACE (DEBUG before O1).
"""

import argparse
import collections
import datetime as dt
import json
import os
import re
import statistics

TS = re.compile(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z")
KIND = re.compile(r'kind[=:] ?"([^"]+)"')


def field(line, name):
    m = re.search(r"\b" + name + r'[=:] ?"([^"]*)"', line)
    if m:
        return m.group(1)
    m = re.search(r"\b" + name + r"[=:] ?([^ ,\s]+)", line)
    return m.group(1) if m else None


def rest(line, name):
    m = re.search(r"\b" + name + r"[=:] ?(.*)$", line)
    return m.group(1).strip().strip('"') if m else None


def parse_ts(s):
    return dt.datetime.fromisoformat(s[:26]).replace(tzinfo=dt.timezone.utc).timestamp()


def pct(xs, p):
    if not xs:
        return None
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(p * len(xs)))]


PHASE_KINDS = [
    "TransitionFrontierSyncLedgerStakingPending",
    "TransitionFrontierSyncLedgerStakingSuccess",
    "TransitionFrontierSyncLedgerNextEpochPending",
    "TransitionFrontierSyncLedgerNextEpochSuccess",
    "TransitionFrontierSyncLedgerRootPending",
    "TransitionFrontierSyncLedgerSnarkedSuccess",
    "TransitionFrontierSyncLedgerStagedPartsFetchPending",
    "TransitionFrontierSyncLedgerStagedPartsPeerFetchPending",
    "TransitionFrontierSyncLedgerStagedPartsPeerFetchError",
    "TransitionFrontierSyncLedgerStagedPartsPeerFetchSuccess",
    "TransitionFrontierSyncLedgerStagedPartsPeerInvalid",
    "TransitionFrontierSyncLedgerStagedReconstructPending",
    "TransitionFrontierSyncLedgerStagedReconstructSuccess",
    "TransitionFrontierSyncLedgerStagedReconstructError",
    "TransitionFrontierSyncLedgerRootSuccess",
    "TransitionFrontierSynced",
    "TransitionFrontierSyncBestTipUpdate",
]


def analyse_log(path, start=None, until=None):
    t0 = None
    phases = []
    errors = collections.Counter()
    disconnect = collections.Counter()
    sched = collections.Counter()
    applies = []
    reqs = {}
    kinds_sent = collections.Counter()
    lat = collections.defaultdict(list)
    per_peer = collections.defaultdict(collections.Counter)
    timeouts = collections.Counter()
    none_resp = collections.Counter()
    first_synced = None
    events = []
    phase_abs = []
    with open(path, errors="replace") as f:
        for line in f:
            m = TS.match(line)
            if not m:
                continue
            t = parse_ts(m.group(1))
            if start and t < start:
                continue
            if until and t > until:
                break
            if t0 is None:
                t0 = t
            if "http_server" in line[:120] or "http_request" in line[:120]:
                continue
            km = KIND.search(line)
            kind = km.group(1) if km else None
            if kind is None:
                continue
            if first_synced is not None and t > first_synced + 1:
                # Only the bootstrap: stop counting a second after Synced.
                break
            if kind in PHASE_KINDS:
                extra = ""
                if kind == "TransitionFrontierSyncBestTipUpdate":
                    extra = "h=%s root_snarked=%s" % (
                        field(line, "new_best_tip_height"),
                        (field(line, "new_root_snarked_ledger_hash") or "")[:12],
                    )
                elif kind == "TransitionFrontierSynced":
                    extra = "h=%s" % field(line, "block_height")
                    first_synced = t
                elif "Parts" in kind:
                    extra = " ".join(
                        x
                        for x in [field(line, "peer_id"), field(line, "error")]
                        if x
                    )
                short = (
                    "Synced"
                    if kind == "TransitionFrontierSynced"
                    else kind.replace("TransitionFrontierSyncLedger", "").replace(
                        "TransitionFrontierSync", ""
                    )
                )
                phases.append((t - t0, short, extra))
                phase_abs.append((t, kind))
            if kind == "LedgerService::block_apply":
                applies.append(t - t0)
            if kind.startswith("P2pNetworkScheduler"):
                pass
            elif kind.endswith(("Error", "Timeout", "Rejected", "Invalid")):
                err = field(line, "error")
                errors[(kind, err if err and len(err) < 40 else None)] += 1
            if kind == "P2pDisconnectionInit":
                r = rest(line, "reason") or ""
                r = re.sub(r"12D3\w+|2[a-zA-Z0-9]{50}", "<peer>", r)
                r = re.sub(r"\d+\.\d+\.\d+\.\d+(:\d+)?", "<ip>", r)
                disconnect[r[:110]] += 1
            if kind in ("P2pNetworkSchedulerError", "P2pNetworkSchedulerSelectError"):
                e = rest(line, "error") or ""
                e = re.sub(r"12D3\w+", "<peer>", e)
                sk = field(line, "select_kind")
                sched[(kind.replace("P2pNetworkScheduler", ""), (sk or "")[:20], e[:70])] += 1
            if kind.startswith("P2pChannelsRpc"):
                at = field(line, "time")
                at = int(at) / 1e9 if at and at.isdigit() else t
                rpc_ = field(line, "rpc")
                if rpc_ and rpc_.startswith("Some("):
                    rpc_ = rpc_[5:].rstrip(")")
                events.append((at, kind[len("P2pChannelsRpc"):], field(line, "peer_id"), field(line, "id"), rpc_))
            if kind == "P2pChannelsRpcRequestSend":
                peer, rid, rpc = field(line, "peer_id"), field(line, "id"), field(line, "rpc")
                reqs[(peer, rid)] = (t, rpc)
                kinds_sent[rpc] += 1
                per_peer[peer][rpc] += 1
            elif kind == "P2pChannelsRpcResponseReceived":
                peer, rid, rpc = field(line, "peer_id"), field(line, "id"), field(line, "rpc")
                r = reqs.pop((peer, rid), None)
                if r:
                    lat[r[1]].append(t - r[0])
                    if rpc in (None, "None"):
                        none_resp[r[1]] += 1
            elif kind == "P2pChannelsRpcTimeout":
                peer, rid = field(line, "peer_id"), field(line, "id")
                r = reqs.pop((peer, rid), None)
                timeouts[r[1] if r else "?"] += 1
    return {
        "t0": t0,
        "phases": phases,
        "errors": errors,
        "disconnect": disconnect,
        "sched": sched,
        "applies": applies,
        "kinds_sent": kinds_sent,
        "lat": lat,
        "per_peer": per_peer,
        "timeouts": timeouts,
        "none_resp": none_resp,
        "unanswered": collections.Counter(v[1] for v in reqs.values()),
        "events": events,
        "phase_abs": phase_abs,
        "end": first_synced,
    }


def windows(a):
    """Phase windows (name, start, end) in absolute seconds."""
    ph = a["phase_abs"]
    if not ph:
        return []

    def first(kind, after=0.0):
        return next((t for t, k in ph if k == kind and t >= after), None)

    def last(kind, before=float("inf")):
        ts = [t for t, k in ph if k == kind and t <= before]
        return ts[-1] if ts else None

    start = ph[0][0]
    out = []
    st = first("TransitionFrontierSyncLedgerStakingSuccess")
    ne_p = first("TransitionFrontierSyncLedgerNextEpochPending")
    ne = first("TransitionFrontierSyncLedgerNextEpochSuccess")
    rp = first("TransitionFrontierSyncLedgerRootPending")
    rs = first("TransitionFrontierSyncLedgerRootSuccess")
    sync = first("TransitionFrontierSynced")
    if st:
        out.append(("staking ledger", start, st))
    if ne_p and ne:
        out.append(("next epoch ledger", ne_p, ne))
    if rp:
        rsn = first("TransitionFrontierSyncLedgerSnarkedSuccess", rp)
        if rsn:
            out.append(("root snarked ledger", rp, rsn))
            rec = last("TransitionFrontierSyncLedgerStagedReconstructPending", rs or float("inf"))
            if rec:
                out.append(("staged parts (all attempts)", rsn, rec))
                if rs:
                    out.append(("last reconstruct", rec, rs))
    if rs and sync:
        out.append(("blocks", rs, sync))
    return out


def print_windows(a):
    ws = windows(a)
    if not ws or not a["events"]:
        return
    t0 = a["t0"]
    print("\n## RPCs by phase (sent in the window; prototype logging)")
    ev = sorted(a["events"])
    pending = {}
    done = []  # (send_t, peer, rpc, outcome, latency)
    last_resp = {}
    turnaround = []  # (send_t, gap since the peer's previous response)
    for t, k, peer, rid, rpc in ev:
        if k == "RequestSend":
            if peer in last_resp:
                turnaround.append((t, t - last_resp.pop(peer)))
            pending[(peer, rid)] = (t, rpc)
        elif k in ("ResponseReceived", "Timeout"):
            r = pending.pop((peer, rid), None)
            if r:
                outcome = "timeout" if k == "Timeout" else ("empty" if rpc in (None, "None") else "ok")
                done.append((r[0], peer, r[1], outcome, t - r[0]))
            last_resp[peer] = t
    for name, ws_, we in ws:
        rows = [d for d in done if ws_ <= d[0] < we]
        dur = we - ws_
        print(f"\n{name}: {ws_ - t0:.1f}-{we - t0:.1f}s ({dur:.1f}s)")
        by = collections.defaultdict(list)
        for d in rows:
            by[d[2]].append(d)
        for rpc, ds in sorted(by.items(), key=lambda kv: -len(kv[1])):
            ok = [d[4] for d in ds if d[3] == "ok"]
            peers = collections.Counter(d[1] for d in ds)
            top = peers.most_common(1)[0][1] if peers else 0
            busy = sum(d[4] for d in ds)
            line = (
                f"  {rpc:42s} {len(ds):6d} req, {len(peers):3d} peers (top {top}), "
                f"empty {sum(d[3] == 'empty' for d in ds)}, timeout {sum(d[3] == 'timeout' for d in ds)}, "
                f"{len(ds) / dur:6.1f}/s, mean in flight {busy / dur:5.1f}"
            )
            if ok:
                line += f", latency med {statistics.median(ok):.3f}s p90 {pct(ok, .9):.3f}s max {max(ok):.2f}s"
            print(line)
        ta = [g for t, g in turnaround if ws_ <= t < we]
        if ta:
            print(
                f"  our turnaround (response to next request on the same peer): "
                f"med {statistics.median(ta) * 1000:.1f}ms p90 {pct(ta, .9) * 1000:.1f}ms "
                f"mean {statistics.mean(ta) * 1000:.1f}ms"
            )


def print_log(a):
    print("## Phases (seconds from the first log line)")
    for t, k, e in a["phases"]:
        print(f"{t:8.1f}  {k:45s} {e}")
    ap = a["applies"]
    if ap:
        print(f"\nblock_apply lines: {len(ap)}, first {ap[0]:.1f}, last {ap[-1]:.1f}")
    print("\n## Errors by kind")
    for (k, e), n in a["errors"].most_common():
        print(f"{n:6d}  {k} {e or ''}")
    print("\n## Disconnections we initiated, by reason")
    for r, n in a["disconnect"].most_common():
        print(f"{n:6d}  {r}")
    print("\n## Connection errors")
    for (k, sk, e), n in a["sched"].most_common(20):
        print(f"{n:6d}  {k} {sk} {e}")
    if a["kinds_sent"]:
        print("\n## RPC requests we sent (prototype logging)")
        for rpc, n in a["kinds_sent"].most_common():
            ls = a["lat"].get(rpc, [])
            print(
                f"{n:6d}  {rpc:45s} answered {len(ls)} (empty {a['none_resp'][rpc]}), "
                f"timeouts {a['timeouts'][rpc]}, unanswered {a['unanswered'][rpc]}; "
                + (
                    f"latency median {statistics.median(ls):.3f}s p90 {pct(ls, .9):.3f}s max {max(ls):.3f}s"
                    if ls
                    else ""
                )
            )
        print("\n## Requests per peer")
        rows = sorted(a["per_peer"].items(), key=lambda kv: -sum(kv[1].values()))
        for peer, c in rows:
            print(f"{sum(c.values()):6d}  {peer}  " + ", ".join(f"{k}={v}" for k, v in c.most_common()))


def analyse_samples(d, t0):
    path = os.path.join(d, "samples.jsonl")
    if not os.path.exists(path):
        return
    rows = []
    with open(path) as f:
        for line in f:
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    if not rows:
        return
    t0 = t0 or rows[0]["t"]
    print("\n## Samples: peers, in-flight requests, RSS and CPU by status")
    by_status = collections.OrderedDict()
    prev = None
    for r in rows:
        s = (r.get("sync") or {}).get("status") or "?"
        p = r.get("peers") or {}
        ready = p.get("ready") if isinstance(p, dict) else None
        prog = r.get("progress") if isinstance(r.get("progress"), dict) else {}
        inflight = len(prog.get("messages_stats") or {})
        th = r.get("threads") or {}
        d_cpu = collections.Counter()
        if prev and th and prev.get("threads"):
            for tid, (comm, cpu) in th.items():
                pc = prev["threads"].get(tid)
                d_cpu[comm] += cpu - (pc[1] if pc else 0)
        dt_ = r["t"] - prev["t"] if prev else 0
        e = by_status.setdefault(
            s, {"t": [], "ready": [], "inflight": [], "rss": [], "cpu": collections.Counter(), "dt": 0}
        )
        e["t"].append(r["t"] - t0)
        if ready is not None:
            e["ready"].append(len(ready))
        e["inflight"].append(inflight)
        if r.get("rss_mb"):
            e["rss"].append(r["rss_mb"])
        e["cpu"].update(d_cpu)
        e["dt"] += dt_
        prev = r
    for s, e in by_status.items():
        rd = e["ready"] or [0]
        print(
            f"{s:28s} {e['t'][0]:7.1f}-{e['t'][-1]:7.1f}s  ready peers min/med/max "
            f"{min(rd)}/{statistics.median(rd):.0f}/{max(rd)}  in-flight med "
            f"{statistics.median(e['inflight']):.0f}  RSS max {max(e['rss'] or [0]):.0f} MB"
        )
        if e["dt"] > 0:
            top = sorted(((v / e["dt"], k) for k, v in e["cpu"].items()), reverse=True)[:6]
            print("      CPU cores busy by thread: " + ", ".join(f"{k}={v:.2f}" for v, k in top))
    # Distinct peers that answered anything, from message-progress.
    peers = collections.defaultdict(collections.Counter)
    for r in rows:
        prog = r.get("progress") if isinstance(r.get("progress"), dict) else {}
        for pid, st in (prog.get("messages_stats") or {}).items():
            for k, v in (st.get("responses") or {}).items():
                peers[pid][k] = max(peers[pid][k], v)
    if peers:
        tot = collections.Counter()
        for c in peers.values():
            tot.update(c)
        print(
            f"\nmessage-progress: {len(peers)} distinct peers seen with a request in flight; "
            "highest per-connection response counts summed: " + ", ".join(f"{k}={v}" for k, v in tot.most_common())
        )


def analyse_stats_sync(d, t0):
    path = os.path.join(d, "final-stats-sync.json")
    if not os.path.exists(path):
        return
    with open(path) as f:
        snaps = json.load(f)
    boots = [s for s in snaps if s.get("kind") == "Bootstrap"]
    if not boots:
        return
    print("\n## /stats/sync bootstrap snapshots (seconds from the first log line)")
    for s in reversed(boots):
        rel = lambda x: None if x is None else round(x / 1e9 - t0, 1)  # noqa: E731
        print(f"snapshot best_tip_received {rel(s['best_tip_received'])} synced {rel(s['synced'])}")
        for name, led in s["ledgers"].items():
            if not led:
                continue
            sn = led.get("snarked") or {}
            st = led.get("staged") or {}
            sn = collections.defaultdict(lambda: None, sn)
            st = collections.defaultdict(lambda: None, st)
            print(
                f"  {name:14s} snarked hashes {rel(sn['fetch_hashes_start'])}-{rel(sn['fetch_hashes_end'])} "
                f"accounts {rel(sn['fetch_accounts_start'])}-{rel(sn['fetch_accounts_end'])}; "
                f"staged parts {rel(st['fetch_parts_start'])}-{rel(st['fetch_parts_end'])} "
                f"reconstruct {rel(st['reconstruct_start'])}-{rel(st['reconstruct_end'])}"
            )
        bl = s["blocks"]
        fs = [b["fetch_start"] for b in bl if b["fetch_start"]]
        fe = [b["fetch_end"] for b in bl if b["fetch_end"] and b["fetch_start"]]
        as_ = [b["apply_start"] for b in bl if b["apply_start"]]
        ae = [b["apply_end"] for b in bl if b["apply_end"] and b["apply_start"]]
        fd = [(b["fetch_end"] - b["fetch_start"]) / 1e9 for b in bl if b["fetch_start"] and b["fetch_end"]]
        ad = [(b["apply_end"] - b["apply_start"]) / 1e9 for b in bl if b["apply_start"] and b["apply_end"]]
        if fs:
            print(
                f"  blocks {len(bl)}: fetched {len(fd)} from {rel(min(fs))} to {rel(max(fe))}, "
                f"per block median {statistics.median(fd):.2f}s p90 {pct(fd, .9):.2f}s max {max(fd):.2f}s"
            )
        if as_:
            print(
                f"  applied {len(ad)} from {rel(min(as_))} to {rel(max(ae))}, sum {sum(ad):.1f}s, "
                f"per block median {statistics.median(ad):.3f}s p90 {pct(ad, .9):.3f}s max {max(ad):.2f}s"
            )
            # Time the apply loop waited for a fetch: gaps between one apply's
            # end and the next one's start.
            seq = sorted((b["apply_start"], b["apply_end"]) for b in bl if b["apply_start"] and b["apply_end"])
            waits = [max(0, (seq[i + 1][0] - seq[i][1]) / 1e9) for i in range(len(seq) - 1)]
            print(f"  waits between applies: sum {sum(waits):.1f}s, max {max(waits or [0]):.1f}s")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("log", help="the node's log")
    ap.add_argument("samples", nargs="?", help="the sampler's output directory (samples.jsonl, final-*.json)")
    ap.add_argument("--start", help="ignore log lines before this UTC time")
    ap.add_argument("--until", help="stop at this UTC time")
    a = ap.parse_args()
    start = parse_ts(a.start) if a.start else None
    until = parse_ts(a.until) if a.until else None
    r = analyse_log(a.log, start, until)
    print_log(r)
    print_windows(r)
    if a.samples:
        analyse_samples(a.samples, r["t0"])
        analyse_stats_sync(a.samples, r["t0"])


if __name__ == "__main__":
    main()
