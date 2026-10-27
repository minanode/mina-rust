#!/usr/bin/env python3
"""Top actions by time from a saved /stats/actions response.

Usage: actions.py final-stats-actions.json [N]

The node attributes to each action the time until the next action starts,
so an action that calls a service synchronously carries that call's time,
and EventSourceWaitForEvents is the state machine thread's idle time.
"""

import argparse
import json

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument("stats_actions", help="a saved /stats/actions response")
ap.add_argument("n", nargs="?", type=int, default=15, help="how many actions to list (default 15)")
args = ap.parse_args()
d = json.load(open(args.stats_actions))["stats"]
n = args.n
rows = []
for k, v in d.items():
    calls = sum(b["total_calls"] for b in v.values())
    dur = sum(b["total_duration"] for b in v.values()) / 1e9
    if calls:
        rows.append((dur, k, calls))
rows.sort(reverse=True)
total = sum(r[0] for r in rows)
idle = sum(r[0] for r in rows if r[1] == "EventSourceWaitForEvents")
print(f"state machine thread: {total:.1f}s accounted, {idle:.1f}s idle, {total - idle:.1f}s busy")
for dur, k, calls in rows[:n]:
    print(f"{dur:8.2f}s {calls:8d} calls {1000 * dur / calls:7.2f} ms each  {k}")
