#!/usr/bin/env python3
"""Ledger roots from OCaml for the mina-tree and mina-node hash test vectors.

OCaml prints the Merkle root of a list of accounts with
`mina ledger hash --plaintext --ledger-file FILE`, where FILE holds an
`Account.t list` sexp. The accounts go to indices 0, 1, 2 and so on, as in
mina-tree's `ledger_root_b58`. The ledger depth comes from the profile:
`MINA_PROFILE=dev` gives 10 and `MINA_PROFILE=mainnet` gives 35.

This script writes one sexp per test vector into OUTDIR and, given the path
of an OCaml `mina` (or a wrapper such as `scripts/ocaml-mina`), prints the
dev and mainnet roots of each. See private implementation notes

Usage: ledger_roots.py OUTDIR [MINA]
"""

import os
import subprocess
import sys

# Public_key.Compressed.empty (x = 0, is_odd = false)
EMPTY_PK = "B62qiTKpEPjGTSHZrtM8uXiKgn8So916pLmNJKDhKeyBQL9TDb3nvBG"
# Receipt.Chain_hash.empty, 2mzbV7WevxLuchs2dAMY4vQBS6XttnCUF8Hvks4XNBQ5qiSGGBQe
RECEIPT_EMPTY = 0x0AB197CE2129A8A3698BF40D0A3F94DB030E34668FB8A787597A4945063C140B
# Mina_numbers.Txn_version.current in 4.0.2
TXN_VERSION_CURRENT = 4


def field(n):
    """A field element's sexp: big-endian hex with a 0x prefix."""
    return "0x%064X" % n


def permissions(txn_version=TXN_VERSION_CURRENT):
    """Permissions.user_default, with the given verification key version."""
    return (
        "((edit_state Signature) (access None) (send Signature) (receive None)"
        " (set_delegate Signature) (set_permissions Signature)"
        " (set_verification_key (Signature %d)) (set_zkapp_uri Signature)"
        " (edit_action_state Signature) (set_token_symbol Signature)"
        " (increment_nonce Signature) (set_voting_for Signature)"
        " (set_timing Signature))" % txn_version
    )


def account(
    pk,
    token=1,
    symbol="",
    balance=0,
    nonce=0,
    delegate=None,
    txn_version=TXN_VERSION_CURRENT,
):
    """An untimed account without zkApp, voting for State_hash.dummy."""
    return (
        '((public_key %s) (token_id %s) (token_symbol "%s") (balance %d)'
        " (nonce %d) (receipt_chain_hash %s) (delegate %s) (voting_for %s)"
        " (timing Untimed) (permissions %s) (zkapp ()))"
        % (
            pk,
            field(token),
            symbol,
            balance,
            nonce,
            field(RECEIPT_EMPTY),
            "()" if delegate is None else "(%s)" % delegate,
            field(0),
            permissions(txn_version),
        )
    )


def vectors():
    pk = "B62qnzbXmRNo9q32n4SNu2mpB8e7FYYLH8NmaX6oFCBYjjQ8SbD7uzV"
    winner = "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg"
    return {
        # mina-tree test_hash_empty
        "empty": [],
        # mina-tree test_hash_account: Account::create()
        "create": [account(pk, balance=10101, delegate=pk)],
        # A control: the same with txn version 3 must give another root.
        "create-txn-version-3": [
            account(pk, balance=10101, delegate=pk, txn_version=3)
        ],
        # mina-tree test_hash_account, second account
        "seb": [account(pk, symbol="seb", balance=10101, nonce=62772)],
        # mina-tree test_hash_genesis_winner_account
        "genesis-winner": [
            account(winner, balance=20000001000, delegate=winner, txn_version=2)
        ],
        # mina-tree test_merkle_path_long: Account::empty() with token ids 0..7
        "merkle-path-8": [account(EMPTY_PK, token=i) for i in range(8)],
        # mina-tree test_ledger_root_1000_accounts and mina-node
        # test_complete_with_empties: token ids 2..1001, balances 0..999
        "accounts-1000": [
            account(EMPTY_PK, token=i + 2, balance=i) for i in range(1000)
        ],
    }


def main():
    if len(sys.argv) not in (2, 3):
        sys.exit(__doc__)
    outdir = sys.argv[1]
    mina = sys.argv[2] if len(sys.argv) == 3 else None
    os.makedirs(outdir, exist_ok=True)
    for name, accounts in vectors().items():
        path = os.path.join(outdir, name + ".sexp")
        with open(path, "w") as f:
            f.write("(" + "\n ".join(accounts) + ")\n")
        if mina is None:
            print(path)
            continue
        roots = []
        for profile in ("dev", "mainnet"):
            out = subprocess.run(
                [mina, "ledger", "hash", "--plaintext", "--ledger-file", path],
                env=dict(os.environ, MINA_PROFILE=profile),
                check=True,
                capture_output=True,
                text=True,
            )
            roots.append(out.stdout.strip().splitlines()[-1])
        print("%-22s dev %s  mainnet %s" % (name, roots[0], roots[1]))


if __name__ == "__main__":
    main()
