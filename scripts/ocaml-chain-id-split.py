"""Split a chain id input traced by ocaml-chain-id-trace.sh into its parts.

The input is the concatenation OCaml's chain_id.ml hashes with Blake2b-256:
the genesis state hash computed with zero total currencies (52 characters),
the hex MD5 digests of the six constraint system digests (6 x 32), the hex
Blake2b hash of the genesis constants (64), and the hex MD5 of the transaction
and network protocol versions (2 x 32), 372 characters in all.

The gdb output has a "LEN n" line before each captured buffer. Every buffer
must be exactly as long as its LEN, and every 372-byte buffer must be the same
input. If `mina` printed its chain id, the recomputed one must equal it.

usage: python3 -I ocaml-chain-id-split.py GDB_OUTPUT
"""

import hashlib
import re
import sys

LENGTH = 372
NAMES = [
    "transaction-merge",
    "transaction-base",
    "zkapp-opt_signed-opt_signed",
    "zkapp-opt_signed",
    "zkapp-proved",
    "blockchain-step",
]


def captured(path):
    """(LEN, buffer) pairs from the gdb output."""
    lines = open(path, encoding="utf-8", errors="replace").read().splitlines()
    pairs = []
    for i, line in enumerate(lines):
        match = re.fullmatch(r"LEN (\d+)", line)
        if match:
            if i + 1 >= len(lines) or not lines[i + 1].startswith('"'):
                sys.exit("no buffer after %r in %s" % (line, path))
            buffer = lines[i + 1].strip()
            if not (buffer.endswith('"') and len(buffer) >= 2):
                sys.exit("buffer after %r is not one quoted string" % line)
            pairs.append((int(match.group(1)), buffer[1:-1]))
    return pairs, lines


def main(path):
    pairs, lines = captured(path)
    for length, buffer in pairs:
        if len(buffer) != length:
            sys.exit(
                "captured %d characters for LEN %d; gdb may have escaped or "
                "compressed the buffer" % (len(buffer), length)
            )
    inputs = {buffer for length, buffer in pairs if length == LENGTH}
    if len(inputs) != 1:
        sys.exit(
            "expected one distinct %d-byte chain id input in %s, found %d "
            "(lengths seen: %s)"
            % (LENGTH, path, len(inputs), sorted({n for n, _ in pairs}))
        )
    line = inputs.pop()

    print("genesis_state_hash (zero currencies) %s" % line[:52])
    for i, name in enumerate(NAMES):
        start = 52 + 32 * i
        print("digest %-28s %s" % (name, line[start : start + 32]))
    print("genesis_constants_hash %s" % line[244:308])
    for label, md5 in (("transaction", line[308:340]), ("network", line[340:372])):
        version = next(
            (v for v in range(256) if hashlib.md5(str(v).encode()).hexdigest() == md5),
            None,
        )
        print("%s_version md5 %s = md5(%s)" % (label, md5, version))
    chain_id = hashlib.blake2b(line.encode(), digest_size=32).hexdigest()
    print("chain_id %s" % chain_id)

    # `mina internal chain-id` prints the chain id without a newline, so gdb's
    # next message may follow it on the same line.
    printed = [m.group(0) for l in lines for m in [re.match(r"[0-9a-f]{64}", l)] if m]
    if printed:
        if printed[-1] != chain_id:
            sys.exit("mina printed chain id %s, recomputed %s" % (printed[-1], chain_id))
        print("chain_id printed by mina matches")


if __name__ == "__main__":
    main(sys.argv[1])
