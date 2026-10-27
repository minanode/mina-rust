#!/usr/bin/env python3
"""P5.3 gate: record each new best tip's scan state summary from our own relay
(completed work ids in the block, job counts per tree), every 15 s, until
OUT.stop exists or the deadline passes. Runs inside the gate's namespace."""
import collections, json, sys, time, urllib.request
out, deadline = sys.argv[1], float(sys.argv[2])
seen = set()
while time.time() < deadline:
    try:
        open(out + ".stop").close()
        break
    except FileNotFoundError:
        pass
    try:
        s = json.load(urllib.request.urlopen("http://127.0.0.1:3660/scan-state/summary", timeout=30))
        key = (s["block"]["height"], s["block"]["hash"])
        if key not in seen:
            seen.add(key)
            trees = []
            for tree in s["scan_state"]:
                c = collections.Counter()
                for j, job in enumerate(tree):
                    c[("base " if j >= 127 else "merge ") + str(job.get("status"))] += 1
                trees.append(dict(sorted(c.items())))
            with open(out, "a") as f:
                f.write(json.dumps({"time": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                                    "height": key[0], "hash": key[1],
                                    "completed_works": s["block"]["completed_works"],
                                    "trees_newest_first": trees}) + "\n")
    except Exception as e:
        with open(out + ".err", "a") as f:
            f.write(f"{time.time():.0f} {type(e).__name__}: {e}\n")
    time.sleep(15)
