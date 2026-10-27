# Mina Rust node

A Rust implementation of the [Mina protocol](https://minaprotocol.com): a native
node with a built-in dashboard, and a web node that runs in the browser. It is
maintained by the community.

This is a community-maintained continuation of
[o1-labs/mina-rust](https://github.com/o1-labs/mina-rust) (Apache-2.0), which
o1Labs has paused upstream. It is not affiliated with o1Labs or the Mina
Foundation. The node was created by o1Labs and the original OpenMina team, and
this work builds directly on theirs.

Documentation: **[minanode.org](https://minanode.org)**.

## Status

Release 0.20.0 runs on Mina devnet. It follows the chain, produces blocks and
runs a SNARK worker. Mainnet support comes later, and the web node is coming
soon.

See the [0.20.0 release notes](docs/release/0.20.0.md), including the known
issues, and the [CHANGELOG](CHANGELOG.md).

## Quick start

The full steps are in the [quickstart](https://minanode.org/docs/quickstart/).

1. Download and check:

   ```sh
   # Choose x86_64 or arm64 for your Linux CPU.
   arch=x86_64
   url=https://github.com/minanode/mina-rust/releases/download/v0.20.0
   curl -fLO "$url/mina-0.20.0-linux-$arch.tar.gz" &&
   curl --fail-early -fLO "$url/SHA256SUMS" \
     -O "$url/SHA256SUMS.sig" -O https://minanode.org/allowed_signers &&
   ssh-keygen -Y verify -f allowed_signers \
     -I release@minanode.org -n file \
     -s SHA256SUMS.sig < SHA256SUMS &&
   sha256sum --check --ignore-missing SHA256SUMS
   ```

   Continue only after a good signature and an `OK` checksum. The
   [install page](https://minanode.org/docs/install/#check) explains the check.

2. Unpack and run:

   ```sh
   mkdir mina-0.20.0
   tar -xzf "mina-0.20.0-linux-$arch.tar.gz" -C mina-0.20.0
   cd mina-0.20.0
   ./mina node --network devnet
   ```

   The node runs in the foreground and stops when you close the terminal. To
   keep it running, see
   [run in the background](https://minanode.org/docs/run-a-node/#service).

3. Open the dashboard at <http://localhost:3000>. For a server, run this tunnel
   on your computer first (your SSH account must allow forwarding):

   ```sh
   ssh -N -L 3000:127.0.0.1:3000 you@server
   ```

Then see [install](https://minanode.org/docs/install/),
[run a node](https://minanode.org/docs/run-a-node/),
[SNARK worker](https://minanode.org/docs/snark-worker/) and
[block producer](https://minanode.org/docs/block-producer/).

## Supported platforms

| Platform     | CPU                                                          | C library           |
| ------------ | ------------------------------------------------------------ | ------------------- |
| Linux x86_64 | BMI2 and ADX (Intel Broadwell or AMD Zen 1, or newer)        | glibc 2.31 or newer |
| Linux arm64  | AArch64 (ARMv8-A), including a Raspberry Pi 4 on a 64-bit OS | glibc 2.31 or newer |

Distributions that meet the glibc floor include Ubuntu 20.04, Debian 11,
Raspberry Pi OS (64-bit) based on Debian 11, Fedora 32, and RHEL, Rocky Linux or
AlmaLinux 9, and their later releases.

Older releases such as Ubuntu 18.04, Debian 10 and RHEL 8 have an older glibc
and are not supported. The arm64 build has been run on a Raspberry Pi 4; a
Raspberry Pi 5 has not been tested yet, in particular with its default 16
KiB-page kernel.

There are no native macOS or Windows builds for 0.20.0. Building from source on
macOS may work, but it is not tested. On macOS and Windows, run the Docker image
with Docker Desktop.

## Docker

The image `ghcr.io/minanode/mina-rust:0.20.0`, for linux/amd64 and linux/arm64,
holds the same `mina` as the release archives and is built from them. To run a
devnet node with its state in `~/mina-state` and the dashboard at
<http://localhost:3000>:

```sh
mkdir -p ~/mina-state
docker run -d --name mina --restart unless-stopped \
  --user "$(id -u):$(id -g)" \
  -p 127.0.0.1:3000:3000 -p 8302:8302 \
  -v ~/mina-state:/data \
  ghcr.io/minanode/mina-rust:0.20.0
```

Keep the dashboard port on `127.0.0.1`. [docker-compose.yml](docker-compose.yml)
runs the same node with Docker Compose; a block producer adds
[docker-compose.block-producer.yml](docker-compose.block-producer.yml). The full
guide is [run with Docker](https://minanode.org/docs/docker/). The image's files
are in [tools/release/docker](tools/release/docker), and its release steps in
[docs/release/DOCKER.md](docs/release/DOCKER.md).

## Requirements

Every role needs Linux as above, a clock kept in sync by NTP and 5 GB of free
disk. The recommended machines are:

- **follower:** 2 cores, 4 GB RAM;
- **SNARK worker:** 4 or more cores, 8 GB RAM;
- **block producer:** 4 or more cores, 8 GB RAM.

Measured memory use and the details are on the
[requirements](https://minanode.org/docs/requirements/) page.

## Build from source

Build the dashboard first, then the node. The repository pins Rust 1.92 and
Node.js 24.11.0:

```sh
git clone https://github.com/minanode/mina-rust mina-rust
cd mina-rust
cd ui && npm ci && npm run build:dashboard && cd ..
CARGO_BUILD_JOBS=8 cargo build --release --locked \
  -p mina-cli --bin mina
```

The binary is `target/release/mina`; the devnet circuits are embedded, so there
is nothing else to download. System packages and toolchain setup are on the
[build from source](https://minanode.org/docs/build-from-source/) page.

## Repository layout

- [crates/cli](crates/cli): the `mina` command-line program.
- [crates/node](crates/node): the node's state machine and logic, with the
  [native](crates/node/native) and [web](crates/node/web) (browser) builds.
- [crates/p2p](crates/p2p) and [crates/p2p-messages](crates/p2p-messages):
  networking (libp2p and WebRTC) and its messages.
- [crates/ledger](crates/ledger): the ledger, transaction logic, staged ledger
  and scan state.
- [crates/snark](crates/snark): proof verification.
- [crates/core](crates/core): types shared by the other crates.
- [crates/web-relay](crates/web-relay) and
  [crates/web-verifier](crates/web-verifier): relaying blocks to browsers and
  verifying block proofs in the browser.
- [libs/redux](libs/redux): the Redux-style state machine framework.
- [ui/dashboard](ui/dashboard): the dashboard built into `mina`.
- [ui/minanode-site](ui/minanode-site): the minanode.org website.
- [ui/design-system](ui/design-system): components shared by the dashboard and
  the website.
- [tools/release](tools/release): the scripts that build the release archives.
- [docs/release](docs/release): release notes and verification instructions.

## Contributing, security and help

**Contributing:** pull requests are welcome. See
[CONTRIBUTING.md](CONTRIBUTING.md).

**Security:** please report security issues privately to
[security@minanode.org](mailto:security@minanode.org), not in a public issue.

**Getting help:** see [reporting a bug](https://minanode.org/docs/bug-reports/).
`mina report` collects what the developers need and opens a pre-filled GitHub
issue for you to review and submit.

## Licence

Apache-2.0; see [LICENSE](LICENSE). Notices for third-party code and data are in
[THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES) and
[THIRD_PARTY_LICENSES](THIRD_PARTY_LICENSES).
