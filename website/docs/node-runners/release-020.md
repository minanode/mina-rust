---
title: Mesa devnet 0.20.0 release guide
sidebar_position: 2
description:
  Native Linux release requirements, archive contents and node options.
---

# Mesa devnet 0.20.0 release guide

The first community release is for **Mesa devnet** and remains experimental.
There is no public download at this repository revision. Once the maintainer
publishes the archives, verify the published `SHA256SUMS` before unpacking. The
archive contains `mina`, `LICENSE`, `THIRD_PARTY_NOTICES`,
`THIRD_PARTY_LICENSES`, `SOURCES/option-ext-0.2.0.crate`, `README.txt` and
`build-info.json`. The notices list the Rust and embedded dashboard packages;
the bundle has their full licence texts and upstream NOTICE files. The
unmodified `option-ext` source archive supplies its MPL-2.0 corresponding
source, with its crates.io SHA-256 recorded in the notices. Run
`./mina --version` and `./mina build-info` after unpacking; both the archive
label and CLI version should say `0.20.0`.

The unpublished x86-64 candidate built from `84ba54f698ff` was tested from an
empty home and work directory on 2026-10-24. It synced to devnet and its SNARK
worker proved work, but its dashboard returns 404 for `/report-a-problem` even
though the current site documents that page. Rebuild the archive from a commit
that includes the report page, then repeat the dashboard and report checks
before publishing. The candidate's `mina report` also prints a loopback IP in
the report body and absolute work-directory paths in collection progress; do not
claim its entire CLI output is free of IPs or user names.

To regenerate all notices, licence texts and MPL source from the locked Rust
graph and a fresh dashboard build, run from the repository root:

```bash
python3 tools/release/generate-notices.py
```

The command installs the locked dashboard npm packages and builds the bundle
first. The release build invokes the generator after its own dashboard build.

The archives are Linux x86-64 and Linux ARM64 only. Both require glibc 2.31 or
newer. The x86-64 binary requires BMI2 and ADX (Intel Broadwell or newer, AMD
Zen 1 or newer); an older CPU may exit with an illegal instruction. The ARM64
binary uses jemalloc built for kernels with 4, 16 or 64 KiB pages. macOS and
Windows archives are not planned for this release.

## Known limitations

- **Devnet only**, and Linux x86-64 and ARM64 only. There are no mainnet
  archives.
- **Hard forks aren't handled yet.** The node doesn't move to a new chain on its
  own; each hard fork will need a new release.
- **Slow native memory creep.** A long-running node's resident memory rises by
  about 15–30 MiB per hour. It is still being investigated (N-mem). A restart
  brings it back down, and the node restores from its frontier store.
- **The browser node comes later.**

## Minimum machine to try

| Role           | CPU and RAM    | Free disk |
| -------------- | -------------- | --------- |
| Follower       | 1 core, 2 GiB  | 5 GB      |
| Block producer | 4 cores, 8 GiB | 5 GB      |
| SNARK worker   | 4 cores, 8 GiB | 5 GB      |

Use stable broadband (10 Mbps suggested) and an NTP-synchronized clock. These
are planning minima, not guarantees; see the
[public requirements](https://minanode.org/docs/requirements/). A synced devnet
node's data measured 237 MB (132 MB of saved frontier store, 70 MB of ledger
accounts, 29 MB of proof cache), and `mina.log` keeps at most 10 files of 20 MB
(200 MB); 5 GB free leaves room for both to grow and for an upgrade. Measured
memory by role, about 1.0–1.7 GiB for a follower, 3 GiB with the SNARK worker on
and 1.9 GiB for a block producer plus a short peak while it proves a block,
comes from pre-release qualification runs. For headroom, use 2 cores/4 GiB for a
follower or 8 cores/16 GiB for a producer or worker. A source build needs
substantially more disk and RAM; see the
[source quick start](mesa-quickstart.md).

## Start and inspect a follower

The native binary includes the Mesa devnet genesis description, verifier indexes
and prover circuits. Devnet operation needs no separate circuit or genesis
download. Choose a private work directory. The HTTP API and dashboard listen on
`127.0.0.1` by default; use an SSH tunnel to view them from another machine. Use
`--http-addr PRIVATE_IP` for one private interface, or `--http-addr 0.0.0.0` for
all IPv4 interfaces. Keep the port private if exposed. The libp2p peer port
remains separate and can be open to peers. A relay serving browser nodes must
expose its HTTP signalling port with `--http-addr 0.0.0.0` or a reverse proxy.

```bash
./mina node --network devnet --work-dir ./mina-data
./mina node status --node http://127.0.0.1:3000
./mina report --help
./mina triage --help
```

Run the status command from another shell after the node starts. `mina report`
collects a redacted diagnostics report; `mina triage` can ask a locally
installed agent to draft an issue. Review the report before submitting it.

## Node options in 0.20.0

### Block producer VRF progress

A native block producer saves evaluated slots in
`<work-dir>/vrf-evaluation/<producer-public-key>-<epoch>.json`. The file records
the epoch seed, staking ledger hash, total currency, a contiguous evaluated slot
range, and won slots with their public VRF outputs and delegators. It does not
contain a private key. It **does reveal future winning slots**; the node writes
it with file mode `0600` in a directory with mode `0700`.

On restart, matching results are replayed to the evaluator without running the
VRF again. A changed producer, seed, staking ledger or total currency causes the
node to evaluate again. Corrupt or partial files are ignored. To clear the
cache, stop the node, remove `<work-dir>/vrf-evaluation/`, and restart it. This
only removes VRF progress; it does not remove the frontier store.

Use `./mina node --help` for the full list. The frontier store is **on by
default** and restores a recent validated tip on restart. `--no-frontier-store`
disables it. `--frontier-store-load-timeout` defaults to 60 seconds and accepts
1–300 seconds. The metrics listener is off until `--metrics-port PORT` is set;
`--metrics-addr` defaults to `127.0.0.1`. `--http-addr` also defaults to
`127.0.0.1`; `--port` still selects its port.

`--log-level` is an alias for `--verbosity` (default `info`). The other OCaml
flag names work too: `--external-port` for `--libp2p-port`, `--run-snark-worker`
for `--run-snarker`, `--snark-worker-fee` for `--snarker-fee` and
`--block-producer-key` for `--producer-key`. Worker control is off until
`--allow-worker-control` is set; use repeatable `--worker-control-origin ORIGIN`
to permit a separate dashboard origin. Keep the HTTP listener private when
enabling worker control.

Browser pages may read the HTTP API when they are served from the node's machine
(`localhost`, `127.0.0.1` or `[::1]`) or from an origin listed with repeatable
`--http-cors-origin ORIGIN`; `--http-cors-origin '*'` allows every page. Open
the dashboard at an IP address or `localhost`, or list the name you open it at,
as `--http-cors-origin http://NAME:PORT`. The public GraphQL API
(`--public-graphql`) and WebRTC signalling answer every page.

The WebRTC relay options are `--webrtc-ice-servers FILE` (JSON ICE server list),
`--webrtc-udp-ports MIN-MAX`, and repeatable `--webrtc-nat-ip ADDRESS`. They
respectively override built-in STUN servers, restrict candidate UDP ports, and
advertise a public address for a 1:1 NAT. See the
[SNARK worker guide](snark-worker.md) and [bug report guide](bug-reports.md) for
their operator procedures.
