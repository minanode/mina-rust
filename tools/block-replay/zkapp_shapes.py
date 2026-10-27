#!/usr/bin/env python3
"""Classify the zkApp commands of precomputed blocks by their segments.

    zkapp_shapes.py <dir with chain.txt and blocks/> [max blocks]

Prints each shape (the authorization of each account update, fee payer first:
S signature, N none, P proof, as `mesa_zkapp_job_fixtures --want` takes them)
with its segments, status, count and first block, then the segment counts.

Segments follow OCaml's Zkapp_command.group_by_zkapp_command_rev for one
command: the fee payer and the account updates in pre-order; a proof-authorised
update is a segment alone (Proved); two consecutive non-proof updates are one
(OptSignedOptSigned); a non-proof update followed by a proof or the end is
OptSigned.
"""
import collections
import json
import subprocess
import sys

root = sys.argv[1]
limit = int(sys.argv[2]) if len(sys.argv) > 2 else None
chain = [l.split() for l in open(f"{root}/chain.txt") if l.strip()]
if limit:
    chain = chain[:limit]


def flatten(forest, out):
    for tree in forest:
        au = tree["elt"]["account_update"]
        auth = au["authorization"]
        kind = auth[0] if isinstance(auth, list) else "Signature"
        out.append({"Proof": "P", "Signature": "S", "None_given": "N"}[kind])
        flatten(tree["elt"]["calls"], out)
    return out


def segments(auths):
    segs = []
    i = 0
    while i < len(auths):
        if auths[i] == "P":
            segs.append("P")
            i += 1
        elif i + 1 < len(auths) and auths[i + 1] != "P":
            segs.append("SS")
            i += 2
        else:
            segs.append("S")
            i += 1
    return segs


counts = collections.Counter()
first = {}
kinds = collections.Counter()
for height, h in chain:
    raw = subprocess.run(["zstdcat", f"{root}/blocks/{height}-{h}.json.zst"], capture_output=True).stdout
    d = json.loads(raw.decode("utf-8", "replace"))
    d = d.get("data", d)
    for part in d["staged_ledger_diff"]["diff"]:
        if part is None:
            continue
        for idx, c in enumerate(part["commands"]):
            if c["data"][0] != "Zkapp_command":
                continue
            z = c["data"][1]
            auths = ["S"] + flatten(z["account_updates"], [])
            segs = segments(auths)
            status = c["status"][0]
            key = (" ".join(segs), status)
            counts[key] += 1
            first.setdefault(key, (int(height), h, idx, "".join(auths)))
            for s in segs:
                kinds[(s, status)] += 1
for k, v in sorted(counts.items(), key=lambda kv: -kv[1]):
    print(v, k, "first:", first[k])
print("segments:", dict(kinds))
