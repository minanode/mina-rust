# Release signing and publication

Use the release owner's machine for signing. The release private key must never
be on the build machine. This procedure uses SSH signatures, namespace `file`
for checksums and Git's `git` namespace for tags. Use OpenSSH 8.2 or newer and
Git with SSH signing support (2.34 or newer). The scripts also need Bash and GNU
`sha256sum`.

Complete the [0.20.1 candidate checklist](#0201-release-candidate-checklist)
before creating the final tag. Commands below describe owner publication work.
An agent preparing a readiness PR must not execute public writes.

0.20.1 is a native-node release: its assets and `SHA256SUMS` contain only the
two Linux archives. Minaweb and the downloadable web archive have a separate
release plan in [web-node-next.md](web-node-next.md). Keep the web download page
hidden for 0.20.1; none of the web gates below qualifies a 0.20.1 asset.

## Set up and back up the keys

Recommended: generate the release key in 1Password as an **SSH Key** item, using
Ed25519. Sign through the
[1Password SSH agent](https://developer.1password.com/docs/ssh/agent/). The
private key is never exported to a key file. Enable the agent in 1Password's
**Settings > Developer**. Find the socket path for your platform in the linked
agent setup instructions; set `SSH_AUTH_SOCK` to that path in the signing shell.
Export only the public key to a local file. Set `RELEASE_PUBLIC_KEY` to its
path. Unlock 1Password and approve each signing request.

Alternatively, set `RELEASE_PRIVATE_KEY` to a new local key path and generate a
file-based key. Enter a strong passphrase when prompted; do not leave it empty:

```sh
ssh-keygen -t ed25519 -C "minanode release signing" -f "$RELEASE_PRIVATE_KEY"
```

**Blocking checklist, before any signing or GitHub key registration:**

- [ ] Confirm that the release key is backed up, for example in 1Password, and
      that the backup can be recovered.
- [ ] Confirm that the automation commit-signing key is backed up, for example
      in 1Password, and that the backup can be recovered.

Stop if either backup is unconfirmed. Do not use the release key or register any
key on GitHub until both items are complete. Keep these keys separate. Only the
release public key belongs in this repository.

Replace the single pending entry in [allowed_signers](allowed_signers) with the
release public key, keeping `release@minanode.org` as its identity. Record its
SHA-256 fingerprint in `releaseSigningFingerprint` in
`ui/minanode-site/content/release.ts`. Obtain the public key and confirm its
fingerprint through a trusted channel before committing it:

```sh
ssh-keygen -lf "$RELEASE_PUBLIC_KEY" -E sha256
```

The site's `public/allowed_signers` symlink serves that same tracked file. Do
not maintain a second copy. For GitHub's Verified tag badge, the release owner
also adds this public key to their account as a **Signing key**. Local tag
verification below uses the tracked signer file.

## 1. Build on the build machine

Build the reviewed release commit. Retain its full commit ID and the
`build-info.json` files for comparison:

```sh
CARGO_BUILD_JOBS=8 tools/regression/lock.sh build -- \
  tools/release/build.sh 0.20.1
```

Build the site with the final-release gate enabled. This is a blocking check: it
fails until the pending signer entry and fingerprint have been replaced. It also
checks that the fingerprint matches the public key:

```sh
RELEASE_FINAL=1 npm run build --prefix ui/minanode-site
```

- [ ] The final-release build passes with the real public key and fingerprint.
- [ ] Archive build metadata identifies the approved commit and a clean tree.
- [ ] For worker releases, optionally run the
      [differential prover smoke gate](../../tools/prover-div-corpus/README.md).
      Set `OCAML_WORKER="$PROVER_DIV_TOOLS/bin/corpus_worker"` for the installed
      user-level `mina-4.0.3-prover-div` tools bundle, or build the adapter
      using the gate README. Keep its runtime libraries outside disposable
      worktrees. Retain its four-way verification results, coverage gaps, timing
      and memory report with the release evidence.
- [ ] Run `tools/uptime-release-gate.py` against the release binary before
      tagging. Supply the OCaml verifier and decoder paths. Retain the passing
      result and every verifier receipt. Follow the
      [uptime release gate](uptime-reporting.md#local-release-gate).
- [ ] Record the outcome of the separate owner-approved
      [live uptime acceptance test](uptime-reporting.md#owner-approved-live-acceptance-plan).
      A local pass is not evidence of production service acceptance.

## 2. Create a draft GitHub release

An agent may upload the unsigned artifacts to a **draft** release. Set `commit`
to the full approved build commit ID first. Do not publish the draft:

```sh
gh release create v0.20.1 --repo minanode/mina-rust \
  --draft --target "$commit" --title "0.20.1" \
  --notes-file docs/release/0.20.1.md \
  dist/mina-0.20.1-linux-x86_64.tar.gz \
  dist/mina-0.20.1-linux-arm64.tar.gz dist/SHA256SUMS
```

## 3. Sign on the release owner's machine

Confirm the two backup checklist items above again before signing. Check out the
reviewed tooling and signer file. Authenticated access is needed to download
draft assets. Download the checksum list and, preferably, both archives:

```sh
mkdir -p release-0.20.1
gh release download v0.20.1 --repo minanode/mina-rust \
  --dir release-0.20.1 --pattern SHA256SUMS --pattern '*.tar.gz'
```

For 1Password, pass the exported public key. The script checks that the agent
lists it, verifies all supplied archives, signs the list, verifies that
signature, creates a signed tag at the explicit build commit and verifies it:

```sh
tools/release/sign-release.sh release-0.20.1 \
  "$RELEASE_PUBLIC_KEY" --tag v0.20.1 --commit "$commit"
```

For a file key, pass `RELEASE_PRIVATE_KEY` instead. To sign only the checksum
list, omit the archives. The script still runs the checksum check, but reports
that no archive contents were checked. Confirm the list against the reviewed
build output. Omit both tag options if a signed tag already exists. Existing
signature files and tags are never overwritten.

Push only the signed tag, then upload the verified checksum signature to the
existing draft. Do not let publication create an unsigned tag:

```sh
git push https://github.com/minanode/mina-rust refs/tags/v0.20.1
gh release upload v0.20.1 release-0.20.1/SHA256SUMS.sig \
  --repo minanode/mina-rust
```

## 4. Verify from a clean machine

Before publishing, use authenticated access to download the draft's archive,
`SHA256SUMS` and `SHA256SUMS.sig`. Obtain `allowed_signers` from the reviewed
site build and compare its fingerprint through a trusted channel. A draft's
public release-download URL will not work yet. Fetch the tag into a clean
checkout of the public repository and verify it:

```sh
git fetch origin tag v0.20.1
git -c gpg.ssh.allowedSignersFile=docs/release/allowed_signers \
  verify-tag v0.20.1
git rev-parse 'v0.20.1^{commit}'
```

Confirm that this is the build commit. In the downloaded assets directory:

```sh
ssh-keygen -lf allowed_signers -E sha256
ssh-keygen -Y verify -f allowed_signers -I release@minanode.org \
  -n file -s SHA256SUMS.sig < SHA256SUMS &&
sha256sum --check --ignore-missing SHA256SUMS
```

- [ ] The real signer and fingerprint match the reviewed source.
- [ ] The tag signature is valid and targets the build commit.
- [ ] The checksum signature is valid and every downloaded archive reports OK.
- [ ] Re-run the final-release site gate; neither signing marker remains.

Stop on any failure. Never replace a failed check with an unsigned artifact.

## 5. Publish

Only the release owner or coordinator approves publication after the checks:

```sh
gh release edit v0.20.1 --repo minanode/mina-rust --draft=false
```

Check the public release downloads and `https://minanode.org/allowed_signers`
from a clean machine using the
[install guide](https://minanode.org/docs/install/#check).

Publishing a release starts the [Docker image workflow](DOCKER.md) when the
tagged commit contains it. A tag older than the workflow, such as `v0.20.0`,
starts nothing: run the workflow by hand with the version, the release commit
and `push` ticked. The
[manual image procedure](DOCKER.md#manual-image-publication) builds
`ghcr.io/minanode/mina-rust` from the same signed archives without it.

Signature formats and configuration:
[OpenSSH](https://man.openbsd.org/ssh-keygen#ALLOWED_SIGNERS) and
[Git](https://git-scm.com/docs/git-config#Documentation/git-config.txt-gpgformat).

## Published site switch

The workspace and archive candidate version is 0.20.1. The site's `release`
object in `ui/minanode-site/content/release.ts` continues to advertise published
0.20.0 archives and its matching Docker digest. `candidateVersion` records the
prepared version; it does not select downloads. `RELEASE_FINAL=1` validates the
signer and content, and does not switch versions.

After the owner publishes and verifies the signed 0.20.1 archives and Docker
image, update these together before deploying the site:

1. Set `release.version` to `candidateVersion`, change both `downloadUrl` and
   `notesUrl` to `v0.20.1`, and replace `release.dockerDigest` with the verified
   0.20.1 image index digest. Never reuse the 0.20.0 digest for 0.20.1.
2. Change the fallback image in `docker-compose.yml` to
   `ghcr.io/minanode/mina-rust:0.20.1`. The site serves this file through its
   `public/docker/` symlink, so the default remains 0.20.0 until publication.
3. Run the site build, typecheck and content checks below, including
   `RELEASE_FINAL=1`, and verify the archive, checksum, notes and image URLs.
   The archive names and Docker examples derive from the `release` object.

The Docker workflow requires an explicit version and verified commit as inputs;
its example now uses 0.20.1. A packaging dry run does not publish an image or
change the running site's downloads. Unsigned dry-run archive hashes must not
replace the public release-note checksum placeholder.

## 0.20.1 release-candidate checklist

Every unchecked item is a pending gate. Historical results do not qualify a new
artifact. Retain the command, full source commit, binary SHA-256, result and
cleanup receipt for each run. Set `EVIDENCE` to a new private output directory.
Set `RELEASE_COMMIT` to the approved public build commit and `RELEASE_BINARY` to
the unpacked archive's executable. Keep private and exported commit IDs
separate. An inventory audit is not publication approval.

### Source and documentation

- [ ] Coordinator: confirm the candidate includes the reviewed uptime
      release-gate change, retry recovery, produced-block notification fixes and
      the [uptime operator guide](uptime-reporting.md). Reconcile overlapping
      release-note edits before freezing the candidate.
- [ ] Coordinator: set the workspace version to 0.20.1 and refresh the release
      archive README and smoke expectations for the qualified worker scope.
      Refresh the site's version, download URLs and generated CLI reference from
      the candidate. Keep beta, work-sales and uptime-acceptance caveats. Review
      the claims ledger.
- [ ] Agent: verify documentation, site output and CLI freshness:

```sh
make format-md
make check-md
make fix-trailing-whitespace
make check-trailing-whitespace
npm ci --prefix ui --no-audit --no-fund
npm run build --prefix ui/minanode-site
npm run typecheck --prefix ui/minanode-site
npm run check --prefix ui/minanode-site
RELEASE_FINAL=1 npm run build --prefix ui/minanode-site
CARGO_BUILD_JOBS=8 tools/regression/lock.sh build -- \
  cargo test --locked --release -p mina-cli cli_reference_is_current
```

The final-release site build must have the real public signer and matching
fingerprint. Update generated CLI Markdown and JSON together using the
`mina internal cli-reference` command if the freshness test fails.

### Native regression and proof gates

- [ ] Agent: run the native regression suites on the candidate. Classify every
      failure before approval. Run relevant ignored expensive tests separately.

```sh
CARGO_BUILD_JOBS=8 tools/regression/lock.sh build -- \
  cargo test --locked --release -p mina-cli -p mina-node \
  -p mina-node-native -p mina-tree -p mina-p2p --lib --bins
CARGO_BUILD_JOBS=8 tools/regression/lock.sh build -- \
  cargo test --locked --release -p mina-tree --features embedded-circuits \
  circuit_blobs
CARGO_BUILD_JOBS=8 tools/regression/lock.sh build -- \
  cargo test --locked --release -p mina-tree --no-default-features circuit_blobs
```

- [ ] Agent: run the private proof, verifier-index, replay and live-network
      qualification gates kept with the release evidence. Record each run's
      isolation setup, binary provenance, result and cleanup receipt.

### Uptime release gate

- [ ] Agent on a suitable Linux host: run the local gate against the actual
      release artifact. Use the qualified OCaml 4.0.3 reference tools built from
      the pinned fixture recipe. Choose free ports assigned by the host. The
      gate uses a devnet node, a fresh unfunded key and disk-only reports. It
      needs user systemd and at least 55 GiB free. It stops its own unit and
      deletes its credentials. Do not run against a shared node.

```sh
python3 -B tools/uptime-release-gate.py \
  --binary "$RELEASE_BINARY" --commit "$RELEASE_COMMIT" \
  --verifier "$OCAML_DELEGATION_VERIFY" --inspect "$OCAML_UPTIME_INSPECT" \
  --http-port "$HTTP_PORT" --libp2p-port "$LIBP2P_PORT" \
  --output "$EVIDENCE"
```

Require three reports and two complete 450-second intervals after the first
observed report. Every saved report must pass OCaml blockchain-proof,
transaction-proof, SOK and HTTP-signature checks. At least one must carry work.
Require zero uptime HTTP attempts and a successful cleanup receipt. Any rejected
report fails, including a terminal zkApp SOK rejection. Do not filter failed
reports out. Earlier version-0.20.0 gate runs do not satisfy this item.

- [ ] Owner: approve the release's uptime claim. Live service acceptance is a
      separate optional run under the uptime guide's owner-approved plan. It
      needs explicit authorization, policy confirmation and retained responses.
      Without it, keep the untested-acceptance and no-credit claims in the
      notes.

### Export and release archives

- [ ] Agent: audit the exact committed private candidate. Set `EXPORT_AUDIT` to
      a new private directory. Require `audit_passed: true` and zero unresolved
      findings. Check that handover docs, component summaries, research and
      private run evidence are excluded. Review every changed waiver. The
      coordinator supplies `EXPORT_TOOL`, the path to the reviewed private
      export script. That preparation tool is not distributed in the public
      source tree. Run it from the private release checkout.

```sh
python3 -B "$EXPORT_TOOL" HEAD --inventory-only --output "$EXPORT_AUDIT"
```

- [ ] Owner/coordinator: approve the export and public commit mapping. Inspect
      the exported tree and allowed workflows before authorizing public writes.
      Never build release archives from the private source commit and label them
      as the public commit.
- [ ] Agent on an isolated release builder: build both Linux architectures from
      the clean approved public checkout. Compare regenerated licence material.
      Retain both `build-info.json` files. Require version 0.20.1, clean tree,
      the approved commit and embedded circuits.

```sh
CARGO_BUILD_JOBS=8 tools/regression/lock.sh build -- \
  tools/release/build.sh 0.20.1
tools/release/smoke.sh dist/mina-0.20.1-linux-x86_64.tar.gz
tools/release/smoke.sh dist/mina-0.20.1-linux-arm64.tar.gz
(cd dist && sha256sum --check SHA256SUMS)
```

Run archive runtime checks on matching architectures. A cross-architecture ELF
check alone is insufficient. Inspect archive metadata, README, licences and
checksums. Replace the release-notes checksum placeholder only with these bytes.
Use the host's node scheduling rules for smoke runs.

On a host already running a node, `SMOKE_NO_NODE=1 tools/release/smoke.sh` with
the archive argument checks the executable's version, build metadata and CLI
help without starting another node. Record this as CLI-only smoke; it does not
qualify bootstrap or sync.

### Deferred web build and public relay

These gates belong to the separate web release, not native 0.20.1.

- [ ] Agent: build and typecheck the web UI, then build the complete devnet WASM
      bundle from the clean candidate. Supply a qualified devnet relay address.
      Set `WEB_BUILD` to a new output directory. Include devnet prover assets
      when qualifying role screens.

```sh
npm run build:minaweb --prefix ui
npm run typecheck --prefix ui/minaweb
npm run test --prefix ui/minaweb
NETWORK=devnet UI=minaweb RELAY="$RELAY_ADDRESS" \
  PROVER_CIRCUITS="$DEVNET_PROVER_CIRCUITS" CARGO_BUILD_JOBS=8 \
  tools/regression/lock.sh build -- \
  tools/regression/webnode/build.sh "$WEB_BUILD"
BASE="$WEB_ORIGIN" PAGE=/ AUTOSTART=1 UNTIL_SYNCED=30 \
  OUT="$BROWSER_EVIDENCE" node tools/regression/webnode/run.mjs 15
```

Serve the built directory with the required isolation headers on an assigned
static port. Configure `CHROME` and `PUPPETEER_DIR` first. Require a connected
peer and a synced tip within one block of the relay. A successful process exit
alone does not prove sync.

- [ ] Owner: install a qualified relay build containing the restricted privacy
      defaults before public minaweb use. Do not enable
      `--webrtc-signaling-relay` on that relay. Retain binary provenance and a
      rollback bundle. Follow [MINAWEB.md](MINAWEB.md) for owner DNS, TLS,
      firewall, TURN, immutable upload and deployment checks. Run its external
      direct and forced-TURN gate:

```sh
RELAY_HTTP="$PRIVATE_RELAY_HTTP" python3 -B tools/deploy/minaweb/check.py \
  "$LAUNCH_EVIDENCE" --turn-ip "$VPS_PUBLIC_IPV4"
```

These launch checks gate public minaweb deployment. They do not gate a native
release that makes no public-service launch claim.

### Owner signing, fresh installation and Docker

- [ ] Owner: confirm both signing-key backups and fingerprints, then sign the
      tag and checksum list using steps 2 through 4 above. Verify from a clean
      machine. Do not publish an unsigned or unverified artifact.
- [ ] Agent with owner-provided disposable VMs: rehearse the install guide on
      fresh Linux x86-64 and ARM64 hosts using the candidate archives. Use the
      authenticated draft download commands above before publication. Run:

```sh
ssh-keygen -Y verify -f allowed_signers -I release@minanode.org \
  -n file -s SHA256SUMS.sig < SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
./mina --version
./mina build-info
./mina node --help
./mina node --network devnet --work-dir "$NODE_DATA" \
  --port "$HTTP_PORT" --libp2p-port "$LIBP2P_PORT"
```

Confirm sync against an independent OCaml tip, stop only the owned node, then
restart with the same state and keys. Exercise the documented worker setup and
uptime dry run with throwaway keys. Record architecture, commands, tip and
restart results. Historical 0.20.0 fresh-VM runs do not qualify 0.20.1.

- [ ] Owner: enable Actions/package permissions and rehearse the Docker image
      workflow on a signed published pre-release. Draft assets are unreadable to
      that workflow. Set `RC_VERSION` and `RC_COMMIT` to that rehearsal's
      version and verified public tag commit.

```sh
gh workflow run docker-image.yml -R minanode/mina-rust --ref main \
  -f version="$RC_VERSION" -f commit="$RC_COMMIT" -f push=false
gh run list -R minanode/mina-rust --workflow docker-image.yml --limit 5
gh run view "$DOCKER_RUN_ID" -R minanode/mina-rust --exit-status
```

Require signature, archive metadata and checksum verification plus both image
architectures. After owner publication, require the publish and shared-tag jobs,
record the digest, and test pulls on fresh matching hosts as in
[DOCKER.md](DOCKER.md). Docker execution belongs on an authorized isolated host.

- [ ] Owner: approve final publication only after the blocking gates pass.
      Confirm that neither the checksum placeholder nor an unsupported sales,
      acceptance or launch claim remains. Follow step 5 above. Retain the final
      public commit, signed tag, archive digests and image digest together.

## Separate web release (not 0.20.1)

The web archive has a separate release, output directory and checksum list. The
native builder never calls the web packager. Set `WEB_VERSION` to the separately
approved web workspace version and `WEB_DIST` to a new directory outside the
native 0.20.1 output. Do not build or upload a web asset as 0.20.1.

```sh
export RELAY="$PUBLIC_WEBRTC_MULTIADDR"
nice -n 10 env CARGO_BUILD_JOBS=8 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 \
  tools/regression/lock.sh build -- \
  python3 -B tools/release/build-web.py "$WEB_VERSION" --output "$WEB_DIST"
(cd "$WEB_DIST" && sha256sum --check SHA256SUMS)
```

- [ ] Owner: fill in the real public relay multiaddr once its identity exists,
      following [MINAWEB.md](MINAWEB.md). There is no built-in public peer ID.
      Packaging refuses a missing relay and records the exact address in the
      manifest. Never publish an archive configured for a private test relay.
- [ ] Coordinator: integrate the canonical browser prover-pin changes before the
      release build. The packager decompresses the tracked devnet circuits and
      checks all 24 files against the canonical OCaml SHA-256 manifest. Include
      them in the archive; their compressed size avoids the need for an external
      circuit host. All browser assets are same-origin.
- [ ] Agent: regenerate the web notices when dependencies or WASM features
      change. From a fresh minaweb build made with the release's features, run
      `python3 -B tools/release/generate-notices.py --web-built "$BUILD/browser"     --output tools/release/web-licenses`,
      then commit the generated files. The shared native/web generator reads the
      WASM target's Cargo graph and Rollup's exact retained npm package
      versions, including nested installs. Packaging checks that graph, feature
      selection and every licence text against the committed set and fails on
      drift. The archive includes `THIRD_PARTY_NOTICES`, `THIRD_PARTY_LICENSES`
      and `closure.json`; native notices remain separate. Review the listed
      upstream crates that omit licence files, for which the generator supplies
      shared SPDX texts.
- [ ] Agent: build from the clean approved public commit, record archive size,
      SHA-256 and manifest commit, unpack into a new directory, then run
      `python3 -B serve.py --verify-only`. Start with `python3 -B serve.py` and
      require a fresh headless browser to reach Synced on devnet through the
      packaged relay. Repeat with `--relay` pointing at an owned native relay.
      Check that the override replaces the allowlist, and verify COOP/COEP, WASM
      MIME type, corruption refusal and loopback-only binding. Stop both owned
      processes. Retain the two browser receipts with the release evidence.
- [ ] Owner: upload `mina-web-<version>.tar.gz` and its separate `SHA256SUMS`
      only to the approved web release. Sign and verify that checksum list;
      never append the web archive to native 0.20.1 checksums. The packager
      neither signs nor uploads and refuses to alter a signed checksum set.
- [ ] Owner: keep `ui/minanode-site/app/docs/_run-web-node` private and its
      `content/docs.ts` entry `hidden: true` throughout native 0.20.1. Before
      publishing the separate web release, introduce independent web version and
      download metadata, verify the asset and signed checksums, then rename the
      route to `run-web-node` and remove `hidden: true`. Build and check the
      site again before deploying it. Native publication never unhides it.

### OCaml 4.0.4 tools

The reporter's wire format and worker sources are unchanged in 4.0.4. Keep the
qualified 4.0.3 tool bundles until OCAML-404-TOOL-QUALIFICATION validates the
4.0.4 delegation verifier output changes and prover dependencies. Preserve
fixture provenance; do not relabel existing 4.0.3 results as 4.0.4 runs. The
consensus adapter needs a separate port because its source file changed. The pin
audit tracks these qualification follow-ups.
