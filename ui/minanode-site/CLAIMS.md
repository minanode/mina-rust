# minanode.org claims map

This file keeps the site's existing claims, figures and caveats. Pre-release
measurements are coordinator-supplied evidence. Their research notes and run
receipts are retained privately and are not distributed with the source code.

`npm run check:claims` checks that claim IDs match the built pages and checks
external resource links. A `coordinator:` source is explicitly not a local
measurement check. Passing the public check does not independently validate
those figures. The release owner must review them before publication.

The copy lives in [`content/site.ts`](content/site.ts). Claim IDs appear as
`data-claim` attributes in the built pages. Source descriptions below identify
the subject of the private measurement; they do not link to unavailable files.

## Hero and proof points

### C02 Headline

- Page: home, h1.
- Says: "Client diversity for Mina. A native node in Rust. A full node in the
  browser."
- Sources: `coordinator: private pre-release evidence; Headline`
- Figures: none
- Caveat kept: a description of the project, no numbers.

### C03 Hero lead

- Page: home, under the headline; the page description and the Open Graph and
  Twitter text use the same wording.
- Says: "A community-led second Mina client, checked against OCaml 4.0.2 by
  replaying the chain, checking proofs and comparing fork choice."
- Sources: `coordinator: private pre-release evidence; Hero lead`
- Figures: `4.0.2`, `replay`, `block proof`, `fork-choice`
- Caveat kept: names the three kinds of check instead of claiming "every step"
  or that all differences are caught (private evidence).

### C04 Release status

- Page: home and Docs overview badges, Open Graph image; the beta sentence under
  the Docs overview lead.
- Says: "0.20.0 beta for devnet"; Open Graph adds "available now". The Docs
  overview adds "0.20.0 is a beta release. Run it on devnet and in test setups.
  Expect changes before it is ready for production." The site header carries a
  "Beta" badge with no claim id.
- Sources: `coordinator: DOCS-V launch copy, 2026-10-24`,
  `coordinator: SITE-BETA, 2026-10-26`
- Figures: `0.20.0`
- Caveat kept: Site copy is prepared for publication on minanode.org. Release
  availability is unverified until publish; see C69. The beta sentence is the
  site's one beta warning and appears on the Docs overview only.

### C84 Devnet sync from scratch

- Page: home, first proof point.
- Says: "About 1 min. Devnet sync from scratch. From OCaml peers; about 2 min
  with 12 nodes starting at once."
- Sources: `coordinator: private pre-release evidence; Devnet sync from scratch`
- Figures: `47.04 s`, `50.05 s`, `12/12`, `90.05 s`, `122.06 s`
- Caveat kept: a rounded value, not one measurement. Two STAB1 fresh probes on
  RC4 took 47 s and 50 s (private run receipt); the LR1b crowd of 12 had a
  median of 90.05 s and a worst of 122.06 s, hence "about 2 min" for 12 at once.

### C82 Mainnet sync from scratch

- Page: home, second proof point.
- Says: "Under 3 min. Mainnet sync from scratch. From an empty store, via OCaml
  peers."
- Sources:
  `coordinator: private pre-release evidence; Mainnet sync from scratch`
- Figures: `154.5 s`, `empty store`, `OCaml peers`, `upper bound`, `145.484 s`,
  `under 3 min`
- Caveat kept: "from an empty store". The STAB1 soak on RC4 took at most 154.5
  s, sampled once a minute (private run receipt, private run receipt); the
  MN1-F1 run, sampled every five seconds, took 145.5 s. Before 25 October 2026
  the native build showed "400 s, Mainnet bootstrap" (private evidence, m11).

### C30 Restart to synced

- Page: home, third proof point.
- Says: "Under 20 s. Restart to synced. The node replays its saved chain."
- Sources: `coordinator: private pre-release evidence; Restart to synced`
- Figures: `12.05 s`, `replayed 297 saved blocks`, `11.59`, `10.44`,
  `under 20 s`
- Caveat kept: a bound over three RC4 restarts on devnet: STAB1's scheduled
  SIGTERM restart, 12.05 s (private run receipt), and LR1b's two frontier
  restores, 11.59 s and 10.44 s. Before 25 October 2026 the native build showed
  "16.2–17.3 s, stored state rechecked" (private evidence).

### C85 RAM for a devnet full node

- Page: home, fourth proof point.
- Says: "About 1 GB. RAM for a devnet full node. Typical use."
- Sources:
  `coordinator: private pre-release evidence; RAM for a devnet full node`
- Figures: `870.6`, `842.2`, `0.90 GB`, `1165.7 MiB`, `about 1 GB`
- Caveat kept: "typical" is the median of once-a-minute RSS samples while
  Synced, two devnet nodes on RC4 over about 8 hours (private run receipt,
  private run receipt). RSS was still rising slowly, so this is remeasured on
  the next release candidate (private run receipt). The Requirements page keeps
  its 24 October figures (C64) until then.

### C86 Raspberry Pi 4 sync

- Page: home, fourth proof point, the detail line.
- Says: "A Raspberry Pi 4 syncs in under 5 min."
- Sources: `coordinator: RC4 Raspberry Pi 4 smoke, 2026-10-25`,
  `coordinator: private pre-release evidence; Raspberry Pi 4 sync`
- Figures: `289 s`, `937 MiB`
- Caveat kept: one devnet run on RC4, Synced in 289 s at 937 MiB; no worker or
  producer on a Pi is claimed.

## What it is and node roles

### C09 Why a second client

- Page: home, "What it is" lead.
- Says: "Mina has one working node implementation, OCaml. With a second, a bug
  in one no longer has to be a bug in every node."
- Sources: `coordinator: private pre-release evidence; Why a second client`
- Figures: `one working node implementation`, `no longer has to be a bug`
- Caveat kept: "no longer has to be", the summary's own wording; it does not say
  our checks catch every difference.

### C13 Full node

- Page: home, Full node card.
- Says: "Syncs mainnet and devnet from OCaml peers and follows the tip. 46 of 46
  sampled tips matched public Auro."
- Sources: `coordinator: private pre-release evidence; Full node`
- Figures: `46/46`, `Auro`, `Mainnet and devnet synced`, `Minute-sampled tips`
- Caveat kept: "sampled" for the tip checks. The tip match is one m11 gate run;
  the card's "Working" status is the scorecard's "Working, on main". The card
  was titled "Follower" before 25 October 2026; the mainnet bootstrap figure it
  used to share is now C82.

### C15 Block producer: public devnet blocks

- Page: home, Block producer card, with an explorer link to 579510.
- Says: "Its blocks reach devnet's best chain, from a shared producer key."
- Sources:
  `coordinator: private pre-release evidence; Block producer: public devnet blocks`
- Figures: `bestChain(maxLength: 290)`, `12 of the 290`, `579510`,
  `shared devnet producer key`
- Caveat kept: the shared producer key. No count on the card, so it doesn't go
  stale; 579510 is one example block. At 2026-10-25T03:02Z, 12 of the 290 blocks
  on Auro's best chain were its own, counted by coinbase receiver (private run
  receipt); the producer's `/stats/block_producer` at the same time (private run
  receipt) listed 22 canonical, 6 orphaned and 21 discarded attempts. Replaces
  blocks 577948 and 577952 (private evidence), cited until 25 October 2026.

### C17 Not yet on mainnet

- Page: home, Block producer card.
- Says: "Not yet on mainnet."
- Sources: `coordinator: private pre-release evidence; Not yet on mainnet`
- Figures: `mainnet block production`
- Caveat kept: this is the caveat.

### C18 SNARK worker

- Page: home, SNARK worker card. The numbers are in proof point C08.
- Says: "Proves the transaction SNARK work that block producers buy. Devnet
  producers have bought its bundles."
- Sources: `coordinator: private pre-release evidence; SNARK worker`
- Figures: `bought 121 Rust-worker bundles`, `devnet`
- Caveat kept: devnet. zkApp jobs are not mentioned, so their pending status is
  not needed.

### C21 Web build: browser-signed transactions

- Page: home, web build text ("and signs and sends transactions over WebRTC").
- Says: "signs and sends transactions over WebRTC".
- Sources:
  `coordinator: private pre-release evidence; Web build: browser-signed transactions`
- Figures: `WebRTC`, `578024`
- Caveat kept: "transactions", not zkApp proofs (private evidence).

## Native and web

### C32 Native build

- Page: home, native build text.
- Says: "A native daemon for every role, with a GraphQL API and a feed to
  OCaml's archive."
- Sources: `coordinator: private pre-release evidence; Native build`
- Figures: `Every node role`, `GraphQL`, `archive`
- Caveat kept: the roles' own statuses on the page say where each was shown;
  block production is "Not yet on mainnet" (C17).

### C05 Mesa mainnet blocks replayed

- Page: home, native build, first metric.
- Says: "15,897. Mesa mainnet blocks replayed. Heights 548148–564044; every
  staged-ledger hash matched OCaml."
- Sources:
  `coordinator: private pre-release evidence; Mesa mainnet blocks replayed`
- Figures: `15,897`, `548148`, `564044`
- Caveat kept: the saved height range is named (OC03); replay is not presented
  as proof verification (OC04). It was a home page proof point until 25
  October 2026.

### C08 Rust SNARK bundles bought

- Page: home, native build, second metric.
- Says: "121. SNARK bundles bought. From its worker, by seven devnet producers
  in one test."
- Sources:
  `coordinator: private pre-release evidence; Rust SNARK bundles bought`
- Figures: `121`, `seven`, `578143`, `578149`
- Caveat kept: devnet, and "in one test", so it doesn't read as a live counter.
  It was a home page proof point, with an explorer link to 578144, until 25
  October 2026.

### C40 OCaml syncs from it

- Page: home, native build, third metric.
- Says: "5m55.84s. OCaml syncs from it. A fresh OCaml devnet node, the Rust node
  its only peer; one run."
- Sources: `coordinator: private pre-release evidence; OCaml syncs from it`
- Figures: `5m55.84s`, `fresh OCaml devnet daemon`, `only configured peer`,
  `exactly the Rust peer`
- Caveat kept: devnet, fresh node, our node as its only peer, one run.

### C11 Web node and web build

- Page: home, Web node card and web build text.
- Says: "The full node in a browser tab, with nothing to install." and "The same
  Rust code, compiled to WebAssembly. It syncs mainnet, verifies proofs, and
  signs and sends transactions over WebRTC." The card's status is C83.
- Sources: `coordinator: private pre-release evidence; Web node and web build`
- Figures: `WebAssembly`, `full node in a browser tab`, `syncs mainnet`
- Caveat kept: none needed; "over WebRTC" is C21.

### C83 Web node coming soon

- Page: home, Web node card ("Coming soon" badge, "It follows this native
  release shortly.") and the Get started web node card's "Coming soon" badge.
  The Get started card's repeat of "It follows this native release shortly." was
  removed in the DOCS-STYLE pass (25 October 2026).
- Says: "Coming soon"; "It follows this native release shortly."
- Sources: `coordinator: DOCS-R web node status, 2026-10-25`,
  `coordinator: private pre-release evidence; Web node coming soon`
- Figures: none
- Caveat kept: no date and no link; the web node's address is not live yet.
  Before 25 October 2026 the card's badge said "Working".

### C24 Download

- Page: home, web build, first metric.
- Says: "3.1 MB. Download. WASM, Brotli-compressed."
- Sources: `coordinator: private pre-release evidence; Download`
- Figures: `3.1 MB`, `Brotli`, `not an uncompressed 3 MB WASM binary`
- Caveat kept: compressed transfer, not the binary's size.

### C25 Peak WASM memory

- Page: home, web build, second metric.
- Says: "1,171 MiB. Peak WASM memory. Median, linear memory only."
- Sources: `coordinator: private pre-release evidence; Peak WASM memory`
- Figures: `1,171`, `median peak WASM linear memory`, `not total Chrome memory`
- Caveat kept: median, and linear memory only (not total browser memory).

### C07 Devnet block made in a browser

- Page: home, web build, third metric, with C26.
- Says: "For canonical devnet block 578136".
- Sources:
  `coordinator: private pre-release evidence; Devnet block made in a browser`
- Figures: `578136`, `five linked descendants`
- Caveat kept: the five linked descendants on Auro are the observation behind
  "canonical". Finality is not claimed. It was a home page proof point, with an
  explorer link, until 25 October 2026.

### C26 Block proof

- Page: home, web build, third metric.
- Says: "42.8 s. Block proof. For canonical devnet block 578136, under load."
- Sources: `coordinator: private pre-release evidence; Block proof`
- Figures: `42.8 s`, `under load`
- Caveat kept: under load.

## Get started and footer

### C50 Release state

- Page: home, "Get started" and the three build cards.
- Says: "Run 0.20.0 beta for devnet"; download and open the dashboard. The
  browser node is a separate build, marked "Coming soon" (C83); the source card
  describes building from public source.
- Sources: `coordinator: DOCS-V launch copy, 2026-10-24`
- Figures: `0.20.0`
- Caveat kept: Planned public repository and release availability are unverified
  until publish (C69). The browser build is separate.

### C56 Native release builds

- Page: home, "Get started", native node card.
- Says: "Linux builds for x86-64 and ARM64, with the devnet prover circuits
  inside."
- Sources: `coordinator: private pre-release evidence; Native release builds`
- Figures: `x86-64`, `ARM64`, `travel inside`
- Caveat kept: Archive contents were checked locally. Public availability is
  unverified until publish (C69).

## Docs pages

The Docs pages (`/docs/`, 22 to 24 October 2026) are instructions: commands,
flags, file names and what the dashboard shows, checked against the code and
`website/docs/node-runners/`. Their figures and claims are below, in
`content/docs.ts`. Memory, disk and default figures use private,
coordinator-supplied measurements. Long-run memory figures use coordinator RSS
samples dated 24 October 2026. The light client endpoints page (26 October 2026)
uses private, coordinator-supplied endpoint measurements and live checks. Each
claim below identifies its evidence source; these measurements are not
independently validated by the public checker.

### C60 Platforms

- Page: Docs overview, the lead; Requirements, the operating system and C
  library rows.
- Says: "one program for Linux on x86-64 or ARM64"; "Linux, x86-64 or ARM64";
  "glibc 2.31 or newer".
- Sources: `coordinator: private pre-release evidence; Platforms`
- Figures: `x86-64`, `ARM64`, `glibc floor of 2.31`
- Caveat kept: Linux only; the archives are not published yet (C69).

### C61 x86-64 CPUs

- Page: Requirements, the CPU row and the paragraph under it.
- Says: "Intel Broadwell or AMD Zen 1, or newer"; "mina uses two sets of x86-64
  instructions, BMI2 and ADX. On an older CPU that lacks either, it stops with
  an illegal instruction error."
- Sources: `coordinator: private pre-release evidence; x86-64 CPUs`
- Figures: `BMI2 and ADX`, `Intel Broadwell`, `AMD Zen 1`, `SIGILL`
- Caveat kept: the page gives a command to check a CPU for both instructions.

### C62 Nothing else to install

- Page: Requirements, the other software row.
- Says: "None: the devnet circuits and the dashboard are inside mina".
- Sources: `coordinator: private pre-release evidence; Nothing else to install`
- Figures: `travel inside`, `embeds`, `on by default for the`
- Caveat kept: devnet circuits, named.

### C63 Clock

- Page: Requirements, the clock row.
- Says: "Kept in sync by NTP".
- Sources: `coordinator: private pre-release evidence; Clock`
- Figures: `NTP synchronized`
- Caveat kept: none needed.

### C64 Full node memory

- Page: Requirements.
- Says: "Devnet full node, x86_64: typical 1.2 GB, peak 1.5 GB over 10 h.
  Mainnet full node, x86_64: typical 1.3 GB, peak 2.0 GB over 24 h on an older
  build. Suggested full node: 2 cores, 4 GB RAM."
- Sources: `coordinator: RSS samples, 2026-10-24`
- Figures: `1.2 GB`, `1.5 GB`, `1.3 GB`, `2.0 GB`
- Caveat kept: Typical is median RSS; peak is the highest once-a-minute sample.
  Rounded decimal GB. The mainnet measurement used an older build; this release
  is devnet only.

### C65 Raspberry Pi 4

- Page: Requirements.
- Says: "Raspberry Pi 4, 4 GB, ARM64. Devnet full node, two 3-hour runs: steady
  mean 341 and 344 MiB, steady p95 364 and 368 MiB; peak 570 and 652 MiB while
  catching up."
- Sources: `coordinator: MEM-ARM Pi 4 jemalloc runs 1 and 4, 2026-10-27`
- Figures: `341`, `344 MiB`, `364`, `368 MiB`, `570`, `652 MiB`, `3-hour`
- Caveat kept: the two jemalloc runs of five (the release allocator). Steady is
  from 30 minutes after sync to the end of each run, sampled every 30 s; the
  figures are each run's mean and p95, not a minimum and maximum. Peak is during
  catch-up, before jemalloc returned memory. A monitored devnet node ran beside
  each test node. No worker or producer on a Pi is claimed.

### C66 SNARK worker memory

- Page: Requirements.
- Says: "Devnet node with SNARK worker: typical 3.2 GB, peak 3.7 GB over 10 h.
  Suggested machine: 4 or more cores, 8 GB RAM."
- Sources: `coordinator: RSS samples, 2026-10-24`
- Figures: `3.2 GB`, `3.7 GB`
- Caveat kept: Typical is median RSS; peak is the highest once-a-minute sample.
  Rounded decimal GB; machine capacity is not RSS.

### C67 Block producer memory

- Page: Requirements.
- Says: "Devnet block producer: typical 2.1 GB, peak 3.4 GB over 10 h, including
  the block prover. Suggested machine: 4 or more cores, 8 GB RAM. A block must
  be proved within its 90-second slot."
- Sources: `coordinator: RSS samples, 2026-10-24`,
  `coordinator: private pre-release evidence; Block producer memory`
- Figures: `2.1 GB`, `3.4 GB`
- Caveat kept: Typical is median RSS; peak is the highest once-a-minute sample
  and can miss a shorter prover peak. Rounded decimal GB.

### C89 Apple M4 under Docker

- Page: Requirements.
- Says: "Apple M4, Docker, ARM64. Devnet full node over 10 minutes synced:
  median 322 MiB, peak 370 MiB. Block producer: 1.14 to 1.20 GiB. SNARK worker:
  1.05 GiB, 1.80 GiB once it loads the zkApp provers."
- Sources:
  `coordinator: Mac install test of the RC6 image, Apple M4 Pro, OrbStack, 2026-10-27`
- Figures: `322 MiB`, `370 MiB`, `10 minutes`, `1.14`, `1.20 GiB`, `1.05 GiB`,
  `1.80 GiB`
- Caveat kept: docker stats every 10 s over about 10 minutes synced, not hours.
  The worker was stopped after it took jobs, so its figure is not a long-run
  peak. One machine, one run each.

### C68 Disk

- Page: Requirements.
- Says: "5 GB free"; "A synced devnet node's data measured 237 MB: 132 MB of
  saved chain state, 70 MB of ledger accounts and 29 MB of proof cache. Logs
  take up to 200 MB more. 5 GB free leaves room for growth and upgrades."
- Sources: `coordinator: private pre-release evidence; Disk`,
  `coordinator: Mac install test of the RC6 image, Apple M4 Pro, OrbStack, 2026-10-27`
- Figures: `237 MB`, `132 MB`, `70 MB`, `29 MB`, `200 MB`, `5 GB`
- Caveat kept: one node's data after about 10 minutes synced, with a worker and
  a producer tried; 5 GB is headroom, not a measured upper bound on future disk
  use.

### C69 Release archives and URLs

- Page: Quickstart download step; Install archive list and release notes.
- Says: Linux x86-64 and ARM64 archives for 0.20.0, checked with a signed
  SHA256SUMS list.
- Sources:
  `coordinator: private pre-release evidence; Release archives and URLs`,
  `coordinator: DOCS-V public release URLs, 2026-10-24`
- Figures: `mina-0.20.0-linux-x86_64.tar.gz`, `mina-0.20.0-linux-arm64.tar.gz`,
  `SHA256SUMS`
- Caveat kept: The planned public repository, download location and release
  notes in content/release.ts are **unverified until publish**. Confirm the
  archives and SHA256SUMS are available at launch. The site is written for
  publication day; it makes no network request to these URLs until clicked.

### C81 Release signing key

- Page: Install, verification step.
- Says: SSH signatures verify the checksum list under release@minanode.org; the
  release key fingerprint is pending until the public key is supplied.
- Sources: `coordinator: SIGN1 SSH signing decision, 2026-10-24`
- Figures: `OpenSSH 8.2`, `release@minanode.org`, `file`
- Caveat kept: The fingerprint and allowed_signers entry are visibly pending.
  RELEASE_FINAL=1 rejects either marker. The site serves the tracked
  docs/release/allowed_signers through a public symlink. Once the key is
  supplied, the content check requires its fingerprint to match the constant.

### C70 Work directory and ports

- Page: Run a node, "The work directory" and "Default ports".
- Says: "The node keeps its data in ~/.mina"; "3000 (--port)"; "8302
  (--libp2p-port)".
- Sources: `coordinator: private pre-release evidence; Work directory and ports`
- Figures: `~/.mina`, `3000`, `8302`
- Caveat kept: none needed; they are the code's defaults.

### C71 Logs

- Page: Run a node, Logs.
- Says: "The log rotates at 20 MB and 10 files are kept, so logs take at most
  200 MB."
- Sources: `coordinator: private pre-release evidence; Logs`
- Figures: `20 MB`, `10 files`, `at most 200 MB`
- Caveat kept: none needed; the code's defaults.

### C72 Worker fee and job selection

- Page: SNARK worker, "Start it with flags" and "Job selection".
- Says: "The fee is in MINA, and the default is 0.1."; "By default it picks one
  of those jobs at random".
- Sources:
  `coordinator: private pre-release evidence; Worker fee and job selection`
- Figures: `0.1 MINA`, `at random`
- Caveat kept: none needed; the code's defaults.

### C73 The fee rule

- Page: SNARK worker, where the dashboard and the flags set the fee.
- Says: "If your key has no account on chain yet, nodes accept your work only
  with a fee of at least 1 MINA, the account creation fee: a block that bought
  it would pay the fee to a new account, which costs 1 MINA." The dashboard
  refuses a lower fee for such a key; with `--run-snarker` the node starts and
  warns once the first proof is refused.
- Sources: `coordinator: private pre-release evidence; The fee rule`,
  `coordinator: RC7-FIXES, OCaml snark_pool.ml prover check and transaction logic sub_account_creation_fee, 2026-10-27`
- Figures: `1 MINA`
- Caveat kept: none needed.

### C74 Confirmed after 15 blocks

- Page: SNARK worker, "Work bought by blocks".
- Says: "Work is pending until 15 blocks are on top of its block, Mina's
  recommended wait for a payment, and confirmed from then on."
- Sources:
  `coordinator: private pre-release evidence; Confirmed after 15 blocks`
- Figures: `Confirmed from 15`, `recommended wait for a payment`
- Caveat kept: "dropped" is named, so confirmed isn't read as final.

### C75 Saved slot checks

- Page: Block producer, "Saved slot checks".
- Says: "The files hold no private key, but they do reveal your future winning
  slots, so the node makes them readable by you only (mode 0600)."
- Sources: `coordinator: private pre-release evidence; Saved slot checks`
- Figures: `vrf-evaluation`, `0600`, `reveal future`, `no key material`
- Caveat kept: the warning itself.

### C76 Known limitations

- Page: Docs overview, "Known limitations".
- Says: "Devnet only."; "Hard forks aren't handled yet: each one will need a new
  release."; "Memory rises slowly on a node that runs for days."; "The browser
  node is a separate build from this native release." The DOCS-STYLE pass (25
  October 2026) removed "We're looking into it." from the memory item and "on
  Linux x86-64 and ARM64" from the first item: through Docker Desktop the node
  also runs on macOS and Windows, and the platforms stay in the overview lead
  (C60).
- Sources: `coordinator: private pre-release evidence; Known limitations`
- Figures: `port at each hard fork`, `No automatic migration`, `memory creep`
- Caveat kept: no rate for the memory rise on the site; the release notes give
  N-mem's. The browser build remains separate from the native release (C50).

### C77 Source-build toolchains

- Page: Build from source, Prepare.
- Says: "The repository pins Rust 1.92 and Node.js 24.11.0. A native build needs
  no nightly toolchain."
- Sources: `coordinator: private pre-release evidence; Source-build toolchains`
- Figures: `1.92`, `24.11.0`
- Caveat kept: the native build is distinct from formatting and WebAssembly.

### C78 Embedded devnet circuits

- Page: Build from source, Build.
- Says: "The devnet prover circuits are in the source and embedded in the
  default build. There is no separate circuit download."
- Sources: `coordinator: private pre-release evidence; Embedded devnet circuits`
- Figures: `4.0.2devnet`, `24`
- Caveat kept: applies to the default native build and devnet circuits.

### C79 Source-build measurements

- Page: Build from source, Build.
- Says: "In one clean-source build on a 16-core x86_64 machine, peak sampled
  build RAM was 4.93 GiB, and the target directory plus toolchains took 4.57
  GiB."
- Sources:
  `coordinator: private pre-release evidence; Source-build measurements`
- Figures: `4.93 GiB`, `4.57 GiB`
- Caveat kept: one build; RAM was sampled, and disk is the final sum, not a
  minimum requirement. The DOCS-STYLE pass (25 October 2026) removed the Cargo
  times (5m22s, then 2m03s), which came from a first attempt that stopped on a
  missing rustfmt and a retry that reused its cache; with them went the caveat
  about that failed attempt.

### C80 Release archive build

- Page: Build from source, Verify.
- Says: "Release archives use tools/release/build.sh with cargo-zigbuild and a
  glibc 2.31 floor."
- Sources: `coordinator: private pre-release evidence; Release archive build`
- Figures: `2.31`, `cargo-zigbuild`
- Caveat kept: the page's native build is not the archive build.

### C85 Light best tip size

- Page: Light client endpoints, `GET /light/best-tip`.
- Says: "The answer is 23,546 bytes in binprot and 32,344 bytes in JSON."
- Sources: `coordinator: private pre-release evidence; Light best tip size`
- Figures: `23,546`, `32,344`
- Caveat kept: none needed. The answer holds a fixed-size header and root state
  and 290 body hashes, so the size is the same at every tip of a full frontier.
  Both sizes were measured again on 26 October 2026.

### C86 Light account size

- Page: Light client endpoints, `GET /light/account/{public_key}`.
- Says: "An answer for an account without a zkApp is about 4.8 KB."
- Sources: `coordinator: private pre-release evidence; Light account size`
- Figures: `4,735–4,867`, `4,776`, `4,866`
- Caveat kept: an account without a zkApp; a zkApp account's answer is larger.

### C87 Light request limits

- Page: Light client endpoints, Limits.
- Says: 16 requests in flight for `/light/*` and the GraphQL path fields
  together, refused at once with `busy`; 32 accounts over all pending light
  requests; 32 accounts per membership query; 10 seconds' wait for the node,
  then `timeout`.
- Sources: `coordinator: private pre-release evidence; Light request limits`
- Figures: `More than 16`, `0 or more than 32 accounts`, `no queue`,
  `would exceed 32`, `waits at most 10 s`
- Caveat kept: none needed; they are the code's constants (`routes/light.rs`,
  `crates/node/src/rpc/light.rs`).

### C88 Next ledger window

- Page: Light client endpoints, "Which ledgers a node holds".
- Says: "On 26 October 2026, the 290 blocks between root and tip spanned 580
  slots of 90 seconds, so next was unavailable for about 14.5 hours after the
  epoch began. If every slot had a block, the gap would be about 7.25 hours."
- Sources: `coordinator: private pre-release evidence; Next ledger window`
- Figures: `580 slots of 90 seconds`, `about 14.5 hours`, `7.25 hours`,
  `06:59:40`
- Caveat kept: dated, and tied to the share of slots with blocks. The page gives
  the rule (until the previous epoch's last block is the root) before the
  figure. A node that ran across the boundary served next 14.5 hours after the
  epoch began.

## Removed in the trim

The site was trimmed to a minimal page on 23 October 2026. These claims were on
the first version and are gone, with later removals listed first:

- On 25 October 2026: the footer's "As of" date (C52) and credit line (C53,
  "Builds on the Rust node begun at o1Labs."), removed at the owner's request.
- On 25 October 2026: the browser mainnet sync proof point (C06, "3.7 min.
  Mainnet sync in a browser", private evidence), a one-off timing.
- The OCaml checks table and its section (C34 to C43): chain replay, mainnet and
  devnet proofs, fork choice, block acceptance, sync from Rust, GraphQL, archive
  feed and live tips. The replay stays as a proof point (C05) and the sync from
  Rust as a native metric (C40).
- "How the work is checked" (C44 to C47) and "What is not claimed" with the
  OCaml-check limits (C48, C54). The hero keeps the bounded wording (C03).
- The hero's explorer link (C01), the three "What it is" cards (C10 to C12), the
  devnet sync line (C14), the isolated-network acceptance line (C16), the
  pending zkApp and browser SNARK lines (C19, C23), the web sync line (C20,
  repeated by C06), the web node's block line (C22, repeated by C07), the block
  578136 receipt and its finality note (C27, C28), the replay time (C31), the
  provenance note (C33), the timings note (C49), the release list (C51) and the
  separate circuits line (C55, now part of C56).

## Left out

Never used, because no source supports them: the mock-up's "1,224/1,224 gates
passing" (it is 1,224 tests), the AI agent verifier and agent sandbox, keys
locked in WASM memory on a Rayon pool, an install button, browser restore times,
the UI latency, 0.46 s signing and 0 MiB heap figures. Also left out by choice:
any comparison with the upstream project, and "before" figures that come from
the upstream code.
