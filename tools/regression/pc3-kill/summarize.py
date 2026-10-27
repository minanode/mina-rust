"""Summarise kill_loop.py runs: one row per iteration, then per case.

  python3 -I tools/regression/pc3-kill/summarize.py RUN_DIR [RUN_DIR...]

An iteration passes when its check start synced, matched the reference at all
three heights, gave the reference's delegators, either restored or listed the
epoch ledgers in its first checkpoint, left no orphaned accounts file and
didn't panic, and its kill start didn't panic either.
"""

import json
import sys


def mb(n):
    return "%.1f" % (n / 1e6)


def main():
    pairs = {}
    for run in sys.argv[1:]:
        for line in open(run + "/kill-loop.jsonl"):
            r = json.loads(line)
            pairs.setdefault((run, r["iteration"]), {})[r["start"]] = r
    cases = {}
    print("| run | it | case | kill | landed | accounts files at kill (MB) | check | synced s | hashes | "
          "delegators | orphans | ledger-accounts MB | work dir MB | pass |")
    print("|---|--:|---|---|---|---|---|--:|---|---|---|--:|--:|---|")
    for (run, it), p in sorted(pairs.items()):
        k, c = p.get("kill"), p.get("check")
        if not k or not c:
            continue
        a = c.get("audit", {})
        ok = bool(c.get("synced_s") and c.get("hashes", {}).get("match")
                  and c.get("delegators", {}).get("match")
                  and (c.get("restored") or c.get("checkpoint", {}).get("match"))
                  and not a.get("orphans") and not c.get("panic") and not k.get("panic"))
        how = k["variant"] if k["variant"] != "ext" else "ext +%s ms" % k.get("delay_ms")
        if k.get("not_reached"):
            how += " (not reached: %s)" % k["not_reached"]
        files = " ".join(mb(v) for v in sorted(k.get("files_at_kill", {}).values())) or "none"
        if k.get("checkpoint_files_at_kill"):
            files += "; store " + " ".join(mb(v) for v in k["checkpoint_files_at_kill"].values())
        print("| %s | %d | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s | %s |" % (
            run.rstrip("/").split("/")[-1], it, k["case"], how, k.get("landed"), files,
            "restored" if c.get("restored") else "bootstrapped", c.get("synced_s"),
            "match" if c.get("hashes", {}).get("match") else "MISMATCH",
            "%s/%s" % (c.get("delegators", {}).get("same"), c.get("delegators", {}).get("keys")),
            len(a.get("orphans", [])), mb(a.get("accounts_bytes", 0)), mb(a.get("home_bytes", 0)),
            "yes" if ok else "NO"))
        s = cases.setdefault(k["case"], {"kills": 0, "pass": 0, "restored": 0, "synced": []})
        s["kills"] += 1
        s["pass"] += ok
        s["restored"] += bool(c.get("restored"))
        if c.get("synced_s"):
            s["synced"].append(c["synced_s"])
    print()
    print("| case | kills | passed | restored | synced s (min-max) |")
    print("|---|--:|--:|--:|---|")
    for case, s in cases.items():
        rng = "%.0f-%.0f" % (min(s["synced"]), max(s["synced"])) if s["synced"] else "-"
        print("| %s | %d | %d | %d | %s |" % (case, s["kills"], s["pass"], s["restored"], rng))


if __name__ == "__main__":
    main()
