#!/usr/bin/env python3
"""Summarise and compare Mina circuit blobs (OCaml dump format used by mina-rust).

For each blob prefix (path without the _gates.json/_rows_rev.bin/... suffix):
  - public input size, number of gates, gates per type
  - number of rows in rows_rev, number of internal vars
  - number of external variables referenced (max External index + 1), i.e.
    PRIMARY_LEN + AUX_LEN as used by crates/ledger/src/proofs/constants.rs
  - a digest of the normalised gate list (typ, wires, coeffs as integers),
    which is independent of the JSON encoding of field elements.

Usage:
  blob_stats.py stats <prefix>...
  blob_stats.py diff <prefixA> <prefixB>   # first differing gate, type deltas
"""
import collections
import hashlib
import json
import sys


def coeff_to_int(c):
    if isinstance(c, str):  # little-endian hex (o1_utils SerdeAs, human readable)
        return int.from_bytes(bytes.fromhex(c), "little")
    return int.from_bytes(bytes(c), "little")  # older: list of bytes


def load_gates(prefix):
    with open(prefix + "_gates.json") as f:
        d = json.load(f)
    gates = []
    for g in d["gates"]:
        wires = tuple((w["row"], w["col"]) for w in g["wires"])
        coeffs = tuple(coeff_to_int(c) for c in g["coeffs"])
        gates.append((g["typ"], wires, coeffs))
    return d["public_input_size"], gates


class Reader:
    def __init__(self, data):
        self.d = data
        self.i = 0

    def byte(self):
        b = self.d[self.i]
        self.i += 1
        return b

    def nat0(self):
        b = self.byte()
        if b < 0x80:
            return b
        n = {0xFE: 2, 0xFD: 4, 0xFC: 8}[b]
        v = int.from_bytes(self.d[self.i : self.i + n], "little")
        self.i += n
        return v

    def int(self):
        b = self.byte()
        if b < 0x80:
            return b
        if b == 0xFF:  # neg int8
            v = self.d[self.i] - 256
            self.i += 1
            return v
        n = {0xFE: 2, 0xFD: 4, 0xFC: 8}[b]
        v = int.from_bytes(self.d[self.i : self.i + n], "little", signed=True)
        self.i += n
        return v

    def v(self):
        tag = self.byte()
        return ("E" if tag == 0 else "I", self.int())

    def field(self):
        # Fp.t bin_io: 32 raw bytes in mina's kimchi backend
        self.i += 32


def load_rows_rev(prefix):
    data = open(prefix + "_rows_rev.bin", "rb").read()
    r = Reader(data)
    nrows = r.nat0()
    max_ext = -1
    n_int_refs = 0
    for _ in range(nrows):
        ncols = r.nat0()
        for _ in range(ncols):
            if r.byte() == 1:
                kind, idx = r.v()
                if kind == "E":
                    max_ext = max(max_ext, idx)
                else:
                    n_int_refs += 1
    assert r.i == len(data), (r.i, len(data))
    return nrows, max_ext, n_int_refs


def load_internal_vars(prefix):
    data = open(prefix + "_internal_vars.bin", "rb").read()
    r = Reader(data)
    n = r.nat0()
    max_ext = -1
    for _ in range(n):
        r.int()  # key
        nterms = r.nat0()
        for _ in range(nterms):
            r.field()
            kind, idx = r.v()
            if kind == "E":
                max_ext = max(max_ext, idx)
        if r.byte() == 1:
            r.field()
    assert r.i == len(data), (r.i, len(data))
    return n, max_ext


def gates_digest(gates):
    h = hashlib.sha256()
    for typ, wires, coeffs in gates:
        h.update(repr((typ, wires, coeffs)).encode())
    return h.hexdigest()[:16]


def stats(prefix):
    pub, gates = load_gates(prefix)
    nrows, max_ext_rows, _ = load_rows_rev(prefix)
    nint, max_ext_int = load_internal_vars(prefix)
    types = collections.Counter(g[0] for g in gates)
    n_ext = max(max_ext_rows, max_ext_int) + 1
    return {
        "name": prefix.rsplit("/", 1)[-1],
        "public_input_size": pub,
        "gates": len(gates),
        "rows_rev": nrows,
        "rows_rev_plus_public": nrows + pub,
        "internal_vars": nint,
        "external_vars": n_ext,
        "aux_len_if_primary=public": n_ext - pub,
        "gates_digest": gates_digest(gates),
        "types": dict(sorted(types.items())),
    }


def main():
    cmd = sys.argv[1]
    if cmd == "stats":
        for p in sys.argv[2:]:
            print(json.dumps(stats(p)))
    elif cmd == "diff":
        a, b = sys.argv[2], sys.argv[3]
        pa, ga = load_gates(a)
        pb, gb = load_gates(b)
        print("public", pa, pb, "gates", len(ga), len(gb))
        ta = collections.Counter(g[0] for g in ga)
        tb = collections.Counter(g[0] for g in gb)
        for t in sorted(set(ta) | set(tb)):
            if ta[t] != tb[t]:
                print("  type", t, ta[t], "->", tb[t], f"({tb[t] - ta[t]:+d})")
        first = next(
            (i for i, (x, y) in enumerate(zip(ga, gb)) if x != y),
            min(len(ga), len(gb)),
        )
        print("first differing gate index", first)
        same_typ = next(
            (i for i, (x, y) in enumerate(zip(ga, gb)) if x[0] != y[0]),
            min(len(ga), len(gb)),
        )
        print("first differing gate type index", same_typ)
        # Count gates whose only difference is coefficients
        coeff_only = sum(
            1 for x, y in zip(ga, gb) if x[0] == y[0] and x[1] == y[1] and x[2] != y[2]
        )
        print("same typ+wires, different coeffs:", coeff_only)
        # Same from the end
        last = 0
        for x, y in zip(reversed(ga), reversed(gb)):
            if x[0] != y[0]:
                break
            last += 1
        print("matching gate types counted from the end:", last)


if __name__ == "__main__":
    main()
