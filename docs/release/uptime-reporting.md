# Uptime reporting in 0.20.1

Uptime reporting is opt-in. Set `--uptime-url` and exactly one of
`--uptime-submitter-key FILE` or `--uptime-submitter-pubkey PUBLICKEY`. Set
`UPTIME_PRIVKEY_PASS` to the key's password. It must be present, even when
empty. The public-key option reads the tracked wallet in the node's
`wallets/store` directory. The key file must have mode 0600.

`--uptime-send-node-commit-sha` includes the first eight characters of the build
commit. `--snarker-fee` sets the work fee without enabling the general worker.
The default fee is 0.1 MINA. Each uptime option also accepts its `MINA_UPTIME_*`
environment variable. The command line takes precedence.

## Reports and timing

The first attempt starts at the next slot boundary. Later attempts start every
five slots, or 450 seconds with Mesa's 90-second slots. Attempts without an
available transition frontier are skipped. Each report contains the whole block
in Base64 Bin_prot, its creation time, the peer ID and optional work and build
commit. The outer request contains version 1, the data, a signature and the
submitter public key.

The best-tip report selects the first transaction job ending at the block's
ledger. It waits for a fresh proof before timestamping and signing. A block
without a suitable job goes without work. A job-list or proving failure skips
the report. The next interval cancels an unfinished attempt.

A local block scheduled within four slots also gets a separate report. The
reporter waits up to three minutes after its slot starts. It sends that block
without work, even if a fork later removes it from the best chain. Like OCaml,
it ignores production events emitted before the wait begins. The first event
after that wait wins, even if another block completes before the reporter
resumes. Best-tip proving and delivery complete before that wait starts. OCaml
4.0.4 broadcasts the breadcrumb before frontier insertion and sends the notified
block: [producer source][producer] and [uptime source][produced-report]. Its
[broadcast latch][bvar] keeps the first event for the waiting consumer.

Each signed body gets up to eight HTTP attempts, four seconds apart. Only HTTP
200 succeeds. HTTP 400, 401, 411 and 413 stop retries. Reporting does not enroll
a key in a delegation program or establish service eligibility.

## Dry run and memory

Add `--uptime-dry-run-dir DIR` to write each signed request instead of sending
it. A URL and submitter are still required. Use a loopback URL for local checks.
The files contain the exact request bodies. `GET /reporting/status` shows zero
HTTP attempts in this mode. Dry run still loads circuits and proves work.

The reporter skips startup, frontier restore and bootstrap until a real,
non-genesis frontier is available. It never saves or submits the temporary
genesis tip. Catch-up from an existing frontier can still report its current
tip, as OCaml does; a saved file alone does not establish that the node has
caught up to the network. Check `mina node status --check` and the reported
height and state hash in `/reporting/status` when assessing readiness.

Allow about 560 MiB extra resident memory for uptime circuits above a producer.
Allow about 700 MiB more when a zkApp job loads its circuits. The optimized
devnet `uptime_producer_memory` test in UPTIME-0201C measured these stages:

| Loaded circuits                       | Process RSS, KiB | Increment, MiB |
| ------------------------------------- | ---------------: | -------------: |
| Producer                              |          929,296 |       baseline |
| Producer and uptime transaction/merge |        1,492,308 |          549.8 |
| Plus zkApp                            |        2,216,712 |          707.4 |

The total increment was 1.23 GiB. These are process RSS readings after circuit
loading, not proving peaks. The test also checked shared transaction-wrap reuse.
Source: `uptime_producer_memory` in `crates/node/native/src/reporting/tests.rs`,
measured in UPTIME-0201C and recorded at commit `a383e3105`. Existing worker
caches can share these circuits. CPU and memory are still shared with production
despite separate proving queues.

## Frontier availability

OCaml checks the frontier broadcast pipe for each best-tip attempt and skips
when it is absent. Bootstrap clears the pipe; the frontier controller publishes
it after initialization or restore, including when it starts catching up. There
is no separate `Synced` requirement in the uptime service. See the [uptime
guard][frontier-guard] and [transition router][frontier-router].

Rust maps this to a non-genesis best tip, completed restore and ledger loading,
no pending ledger recovery, and either synced or catch-up state. Genesis
injection alone sets Rust's sync state to `Synced`, which is insufficient. The
explicit genesis exclusion is stricter than OCaml, whose uptime service can
report genesis if the router exposes it as a real frontier (for example on a new
private chain).

## Intended differences from OCaml 4.0.4

These differences do not change the signed report format.

- Rust keeps the node running if the uptime worker fails. OCaml terminates its
  daemon when the separate worker dies. Uptime must not stop the node. Rust uses
  an in-process worker, so it has no worker-process connection timeout. Failed
  circuit loads retry on later requests with a bounded backoff. [OCaml worker
  lifecycle][worker].
- Rust accepts double-dash long flags only. Clap does not support OCaml's
  single-dash long aliases.
- Rust checks mode 0600 on the key file. It does not require mode 0700 on the
  containing directory. This follows the node's common key loader. Operators can
  still set mode 0700. [OCaml directory check][key-directory].
- Rust sends `User-Agent: mina-rust/<version>` to identify the implementation.
  OCaml sends `ocaml-cohttp/5.0.0`.
- Rust limits each HTTP attempt to 60 seconds, including a 10-second connection
  timeout. This bounds stalled requests. OCaml relies on iteration cancellation.
- Rust does not create `wallets/store` during lookup. Reading a key should not
  create a wallet directory. Duplicate `.pub` matches are sorted by name for
  deterministic lookup; OCaml uses directory order.
- Rust adds environment-variable aliases and a disk-only dry run for operation
  and local verification. OCaml has no equivalent dry-run option.

## zkApp limitation

Rust preserves OCaml 4.0.3/4.0.4's all-zero SOK digest when proving the selected
terminal zkApp segment. This is OCaml-identical behavior, including a known
upstream issue: the [uptime worker][partitioned-worker] passes the segment
statement without binding `Sok_message(fee, submitter)`, and its own [delegation
verifier][sok] rejects that binding. The [4.0.4 worker][worker-404] and
[verifier][sok-404] have the same behavior. We will follow upstream's fix.
Ordinary transaction work still binds its fee and submitter normally.

The local gate makes a narrow, recorded exception for an exactly zero digest
only when OCaml's decoded work statement proves an active zkApp execution ending
in canonical empty local state. Nonzero SOK mismatches, ordinary jobs,
nonterminal segments and ambiguous empty-to-empty statements fail. A node's
job-kind label is not evidence for the exception.

The original signed report must pass block/work round-trip, target/block
equality and HTTP-signature checks. The gate also verifies the work proof
cryptographically under its original zero digest with the tested node binary; a
forged terminal statement cannot obtain the exception. Its full verifier result
must be exactly the known SOK rejection. OCaml checks the block before SOK, but
does not retain populated block-result fields after that rejection. The gate
separately verifies the same block without work for an explicit positive
receipt. This derived verifier input is not a report or a service submission.
Each exception records its statement hash and both verifier cases. It does not
claim that the work and SOK passed full delegation verification. Every other
report must pass full verification. Service acceptance and enrollment remain
separate questions.

## Local release gate

Run the gate before tagging, against the release binary and its full commit ID:

```sh
python3 -B tools/uptime-release-gate.py \
  --binary "$RELEASE_BINARY" --commit "$RELEASE_COMMIT" \
  --verifier "$OCAML_DELEGATION_VERIFY" --inspect "$OCAML_UPTIME_INSPECT" \
  --output "$EVIDENCE"
```

The output directory must be new. The gate needs Linux user systemd and at least
55 GiB free. It generates a fresh unfunded key. It starts one devnet node as a
transient user unit on ports 4330 and 9450. The port options accept any valid
assigned port. The gate checks that both ports are free before startup. No
producer key is used.

Pass `--network mainnet` to run the same disk-only gate on mainnet. This also
selects the mainnet runtime config and signature domain for the OCaml tools, and
takes the shared mainnet node lock. Mainnet needs an `uptime_inspect` built from
the current source with `--signature-kind mainnet` support. Older decoders
remain usable for the default devnet gate. Select the assigned ports with
`--http-port` and `--libp2p-port`.

The gate waits for at least three reports and two full 450-second intervals
after the first report. It stops its unit before verifying every saved file.
OCaml checks blockchain proofs and HTTP signatures for every report, and full
work/SOK verification for every report outside the narrow zkApp exception above.
At least one report must contain work. All other rejections fail the gate.
Investigate a failure; do not discard failing files to make a run pass. The
default total node timeout is 2,400 seconds.

The result records binary provenance, report hashes, runtime and minimum free
space. Verifier receipts remain beside it. The private submitter and password
file are deleted. The node sends no uptime requests. It still connects to devnet
peers. The verifier may fetch public genesis and verification keys. Build the
pinned tools using the [fixture recipe][recipe].

Add `--restart --timeout 5400` to restart the gate's own node with the same
frontier store and require a second full reporting window. The gate records
frontier/reporting samples, requires a completed restore, and verifies reports
from both phases. Every decoded height must exceed the selected network's
post-fork genesis height.

For a bounded smoke check, `--smoke --timeout 900` stops after one saved report
per phase and records `mode: smoke`. It runs the same verification checks but
does not qualify the two-interval release window.

## Owner-approved live acceptance plan

This plan is not part of the local gate. Do not execute it without the owner's
approval for the specific run.

1. Record the release binary's SHA-256 and full build commit. Generate a fresh
   unfunded uptime key with `mina wallet generate`. Set `UPTIME_PRIVKEY_PASS`
   privately. Do not configure a producer or reuse a devnet producer key.
2. Start a mainnet node from that binary in a fresh directory. Set
   `--uptime-url https://uptime-backend.minaprotocol.com/v1/submit` and
   `--uptime-submitter-key`. Omit `--uptime-dry-run-dir`. Use the node lock and
   reserved ports required by the host. Stop only this test's process afterward.
3. Record the UTC time, state hash, work kind, HTTP status and response body for
   each report. Use an owner-approved local recording relay if response bodies
   are needed: the node's normal success log records HTTP status, not the body.
   The relay must forward only this run's reports to the URL above, preserve
   their bytes and record both directions. It must not retry independently.
4. Collect two distinct successful reports at least 450 seconds apart. Proof
   duration can make adjacent report timestamps closer than 450 seconds, so
   continue to a later interval if necessary. Count reports, not HTTP retries.
5. Success means HTTP 200 for both reports, with the response bodies retained
   and no reported validation error. Record any enrollment or policy rejection
   separately from proof or signature failure. Local verifier success alone does
   not establish service acceptance.
6. If OCaml is measured too, record its version, network, fee, work kind and
   submitter policy status. Compare HTTP status and response semantics. Compare
   encodings and signature domains after decoding. Live bodies need not be
   identical: their blocks, peer IDs, timestamps, proof randomness and proving
   times can differ. For a zkApp rejection, compare the SOK error explicitly.

[producer]:
  https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/block_producer/block_producer.ml#L925-L1000
[produced-report]:
  https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/uptime_service/uptime_service.ml#L166-L198
[worker]:
  https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/uptime_service/uptime_snark_worker.ml#L155-L186
[key-directory]:
  https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/crypto/secrets/secret_file.ml#L108-L117
[sok]:
  https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/app/delegation_verify/delegation_verify.ml#L94-L123
[recipe]: ../../tools/uptime-fixtures/full-proof/README.md
[bvar]:
  https://github.com/janestreet/async_kernel/blob/v0.14.0/src/bvar.ml#L35-L49
[frontier-guard]:
  https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/uptime_service/uptime_service.ml#L203-L245
[frontier-router]:
  https://github.com/MinaProtocol/mina/blob/73134bfd3a43c3f4d8ee564b3d04a0c6ed2e0b3c/src/lib/transition_router/transition_router.ml#L370-L497
[partitioned-worker]:
  https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/uptime_service/uptime_snark_worker.ml#L86-L112
[worker-404]:
  https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/uptime_service/uptime_snark_worker.ml#L86-L112
[sok-404]:
  https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/app/delegation_verify/delegation_verify.ml#L94-L123
