#!/usr/bin/env python3
"""Peer table summary from a saved /state/peers response.

Usage: peers_detail.py peers.json
"""

import argparse
import collections
import json
import re

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument("peers", help="a saved /state/peers response, such as final-peers.json")
args = ap.parse_args()
peers = json.load(open(args.peers))
st = collections.Counter(p["connection_status"] for p in peers)
print("statuses", dict(st))
det = collections.Counter()
ports = collections.Counter()
for p in peers:
    if p["connection_status"] == "Connecting":
        d = p.get("connecting_details") or ""
        det[re.sub(r"\(.*", "", d.split("{")[0]).strip()] += 1
    a = p.get("address") or ""
    m = re.search(r"/tcp/(\d+)", a)
    ports[(p["connection_status"], m.group(1) if m else None)] += 1
print("connecting details:", dict(det))
print("status by port:", dict(ports))
for p in peers:
    if p["connection_status"] == "Connecting":
        print(p["address"], (p.get("connecting_details") or "")[:160])
        break
