# Minaweb devnet launch

This is a preparation kit. No account creation, DNS change, upload, purchase or
deployment is performed by the build recipe. The owner runs the production
commands after review. The initial app is the view-only minaweb UI on devnet.

## Release contract

Use a clean, reviewed commit containing the exact seed-relay allowlist and
disabled browser discovery/signalling channels from PR 173, and the WASM
integrity check from PR 181. Native signalling forwarding must remain disabled:
**never pass `--webrtc-signaling-relay` on the public relay**. This does not
disable the direct HTTPS signalling listener used by the page.

The released **0.20.0 binary is not suitable for this public relay**: it
predates PR 173. Build the reviewed main commit with
`tools/deploy/relay/build-relay.sh`, or use a future 0.20.1 release only after
verifying it includes PR 173. A main build may still display version 0.20.0; use
its full embedded commit, binary SHA-256 and CLI help, not the version string
alone. `start-relay.sh` rejects binaries without the opt-in flag. Stock 0.20.0
remains useful for historical comparisons, not launch acceptance.

The bundle contains `browser/`, `upload/`, `manifest.json`, `SHA256SUMS` and
`deploy.sh`. The manifest records the source commit, build settings, every
served file's size and SHA-256. Asset keys are
`releases/FULL_COMMIT/RANDOM_RELEASE_ID/wasm/...` and
`releases/FULL_COMMIT/RANDOM_RELEASE_ID/circuit-blobs/...`. Each build gets a
new namespace, including rebuilds of one commit with different settings. Never
overwrite or delete a release that a deployed page or rollback needs. Rclone's
immutable mode refuses changed remote objects; deploy one release at a time.
Retain the whole local bundle with its manifest.

## Owner setup, in order

1. Create or use the project's Cloudflare account, enable MFA, add the
   `minaweb.org` zone and delegate DNS to its nameservers. Keep the existing
   `mina-node` Pages project separate; create a **Direct Upload** project called
   `minaweb`, production branch `main`.
2. Enable R2 billing and create a **Standard** bucket `minaweb-assets`. Attach
   `assets.minaweb.org` through the bucket's Custom Domains settings. Keep the
   development `r2.dev` endpoint disabled. Create a bucket-scoped R2 read/write
   API token and save its S3 access key pair privately. Configure a private
   `rclone` remote named `r2`, provider Cloudflare, region `auto`, endpoint
   `https://ACCOUNT_ID.r2.cloudflarestorage.com`. Use `rclone config`
   interactively; never put credentials in shell history or this repository. See
   [Cloudflare's rclone setup](https://developers.cloudflare.com/r2/examples/rclone/).
3. Authenticate Wrangler with `npx wrangler login`, or inject a narrowly scoped
   account Pages token from a secret manager. Record the installed Wrangler and
   rclone versions with the release receipt.
4. Provision a public IPv4 VPS: Ubuntu 24.04, x86-64/AVX2, initially 4 vCPU, 8
   GiB RAM and 40 GB disk, with explicit outbound traffic allowance and billing
   alerts. These are starting allocations, not measured capacity guarantees.
   Create `relay.minaweb.org` as a **DNS-only** A record to it. Do not publish
   an AAAA record until IPv6 works end to end.
5. Install the relay and credentials service below; record its public peer ID.
   The owner keeps the identity backup and TURN secret privately.
6. Configure the R2 and Pages headers below, then build, test, upload assets,
   deploy Pages and attach `minaweb.org` in Pages Custom Domains. Redirect `www`
   and the Pages project hostname to the canonical HTTPS origin. Test production
   on `minaweb.org`: preview hostnames are intentionally not on the
   asset/credential CORS allowlist.

## Relay installation

Transfer the kit directories to an owner-controlled VPS staging directory,
preserving `tools/deploy/relay` and `tools/deploy/minaweb`, and run the
installation commands there. The commands in this section run as root on the
**new VPS only**, targeting Ubuntu 24.04 and coturn 4.6.1. Hold coturn off
**before installing the package**: its packaged defaults can otherwise start an
anonymous TURN listener. On a fresh VPS, persistently mask the unit first:

```bash
systemctl mask coturn.service
test "$(systemctl is-enabled coturn.service)" = masked
! systemctl is-active --quiet coturn.service
apt-get update
apt-get install nginx certbot coturn ufw nftables python3
# Before installing any firewall rules, disable the generic flush-on-stop unit.
systemctl disable --now nftables.service
test "$(systemctl is-enabled coturn.service)" = masked
! systemctl is-active --quiet coturn.service
! pgrep -x turnserver
! ss -H -lntu | grep -E ':(3478|5349)[[:space:]]'
```

If coturn is already running, this is not a fresh installation: stop that
installation and investigate its exposure before following this procedure. Keep
the mask through configuration and both firewall installations below. Do not
enable the distribution `nftables.service`: this kit owns a separate table and
unit, without flushing other firewall rules. Do not install Caddy alongside
nginx on ports 80/443. The older Caddy example remains available for private
deployments; this kit uses nginx's built-in rate limiter.

On a clean build checkout:

```bash
nice -n 10 env CARGO_BUILD_JOBS=8 tools/regression/lock.sh build -- \
  tools/deploy/relay/build-relay.sh
tools/regression/check-build-env.sh target/release/mina COMMIT
sha256sum target/release/mina
```

Transfer that binary, `tools/deploy/relay/start-relay.sh`,
`tools/deploy/relay/mina-relay.service`,
`tools/deploy/minaweb/turn-credentials.py` and
`tools/deploy/minaweb/turn-egress.py` into `/opt/mina-relay/releases/COMMIT/`.
Verify the transferred SHA-256. Set scripts and the binary executable,
root-owned and not writable by the service user. Create `current` as a symlink
to this release.

```bash
useradd --system --user-group --home-dir /var/lib/mina-relay \
  --shell /usr/sbin/nologin mina
install -d -o root -g mina -m 750 /etc/mina-relay
install -d -m 755 /opt/mina-relay/releases /var/www/acme
ln -s releases/COMMIT /opt/mina-relay/current
install -m 644 tools/deploy/relay/relay.env /etc/mina-relay/relay.env
install -o root -g mina -m 640 tools/deploy/relay/ice-servers.json \
  /etc/mina-relay/ice-servers.json
runuser -u mina -- test -x /etc/mina-relay
runuser -u mina -- test -r /etc/mina-relay/ice-servers.json
```

Edit `/etc/mina-relay/relay.env`: `NETWORK=devnet`, `EXTERNAL_IP` to the public
IPv4, and `WEBRTC_NAT_IP` only if the interface has a private address behind 1:1
NAT. Keep HTTP 3000 and signalling 3001 on loopback, libp2p 8302, WebRTC UDP
40000–40999, `MAX_PEERS=100` and `EXTRA_ARGS` empty. The relay's ICE list is
`[]`; the public address makes STUN unnecessary on the server.

Generate the identity directly into its private file. Nothing prints the key:

```bash
umask 077
/opt/mina-relay/current/mina keys p2p-key-pair |
  sed -n 's/^secret key: *//p' > /etc/mina-relay/p2p.key
test -s /etc/mina-relay/p2p.key
chmod 600 /etc/mina-relay/p2p.key
! runuser -u mina -- test -r /etc/mina-relay/p2p.key
install -m 644 tools/deploy/relay/mina-relay.service /etc/systemd/system/
systemctl daemon-reload
# Start the relay after the firewall and TURN port checks below.
systemctl enable mina-relay
```

After starting the relay below, read the **public** `P2P identity, peer_id`
startup record using `journalctl -u mina-relay`. Save that ID as
`RELAY_PEER_ID`. Keep the same identity and state directory across updates. Back
up the identity to the owner's secret manager; do not include it in release
bundles.

### Expiring TURN credentials

While coturn is still masked, install its hardened configuration:

```bash
install -o root -g turnserver -m 640 tools/deploy/minaweb/turnserver.conf \
  /etc/turnserver.conf
```

Replace every `<VPS_PUBLIC_IPV4>` with the public IPv4 assigned to the host's
interface. This kit qualifies that topology only; the egress guard rejects
`external-ip` and differing listener/relay/allowed addresses. Qualify NAT or a
separate TURN server before changing these assumptions.

Create the secret and substitute it without printing it, on the VPS:

```bash
python3 - <<'PY'
from pathlib import Path
import os, secrets
os.umask(0o077)
secret = secrets.token_hex(32)
with Path('/etc/mina-relay/turn.secret').open('x') as f:
    f.write(secret + '\n')
p = Path('/etc/turnserver.conf')
s = p.read_text()
assert s.count('<TURN_AUTH_SECRET>') == 1
p.write_text(s.replace('<TURN_AUTH_SECRET>', secret))
PY
install -m 644 tools/deploy/minaweb/turn.env.example /etc/mina-relay/turn.env
# Edit TURN_IPV4 in turn.env; it contains no secret.
install -m 644 tools/deploy/minaweb/mina-turn-credentials.service /etc/systemd/system/
systemctl daemon-reload
! runuser -u mina -- test -r /etc/mina-relay/turn.secret
systemctl enable --now mina-turn-credentials
# coturn remains masked until TLS and both firewalls are ready.
```

The issuer binds only loopback 3002. nginx exposes `/ice-servers.json` over
HTTPS, with no-store and CORS for `https://minaweb.org`. Every page boot follows
a Pages 302 redirect there. The browser receives a random username with a
24-hour expiry and an HMAC credential; the shared secret never leaves the VPS.
The unauthenticated endpoint is deliberately public: origin checks are not
authentication. Quotas, request limits and the peer allowlist bound abuse.

Coturn's peer ACL restricts IP addresses, **not destination ports**, including
in the targeted
[coturn 4.6.1](https://github.com/coturn/coturn/blob/4.6.1/README.turnserver).
The separate nftables output guard permits allocation traffic only to the relay
IPv4's WebRTC UDP range. It permits UDP replies from the TURN listener port,
then drops all other coturn UDP output, including same-host traffic.
`min-port`/`max-port` select allocation source ports; they do not restrict
peers. The guard uses the
[originating socket UID](https://netfilter.org/projects/nftables/manpage.html),
so the systemd drop-in explicitly starts coturn as `turnserver` before it opens
sockets. Do not run other services as that user. TCP control transport remains
available; `no-tcp-relay` prohibits TCP allocations.

Coturn permits at most 200 allocations total and 4 per username. Random
usernames do not make the per-user quota a per-human limit. TCP transport to
TURN is allowed; TCP allocations are not. Allocation lifetime is capped at 600
seconds. No packet-dropping bandwidth cap is configured: previous measurements
found that it caused channel resets. TURN over TLS/443 is not supplied: it needs
another IP or a TLS router.

The page renews expiring coturn credentials before expiry (20% of the remaining
TTL, capped at five minutes). It updates current and future peer configurations
without closing connections or forcing ICE renegotiation. Issuer failures retry
with exponential backoff capped at one minute; expired TURN credentials are
excluded from new connections, while established connections are left running.
Returning to a suspended tab triggers a due refresh. A suspended browser or an
unreachable issuer can still delay reconnects. Rotate a compromised secret in
both coturn and the credential service, restart both, and tell affected visitors
to reload. This can interrupt existing allocations.

### TLS and firewall

Before enabling HTTPS, start the HTTP-only ACME virtual host. Use webroot
authentication for issuance and renewal so nginx keeps serving port 80.

```bash
install -m 644 tools/deploy/minaweb/acme.nginx.conf /etc/nginx/conf.d/minaweb.conf
nginx -t
systemctl enable --now nginx
systemctl reload nginx
certbot certonly --webroot -w /var/www/acme -d relay.minaweb.org \
  --email OWNER_EMAIL --agree-tos
install -m 644 tools/deploy/minaweb/relay.nginx.conf /etc/nginx/conf.d/minaweb.conf
nginx -t
systemctl reload nginx
SSH_FROM=OWNER_CIDR tools/deploy/relay/firewall.sh
ufw status verbose
install -m 644 tools/deploy/minaweb/mina-turn-egress.service /etc/systemd/system/
install -d -m 755 /etc/systemd/system/coturn.service.d
install -m 644 tools/deploy/minaweb/coturn-egress.conf \
  /etc/systemd/system/coturn.service.d/egress.conf
systemctl daemon-reload
systemctl enable --now mina-turn-egress
nft list table inet mina_turn
# Only now may the hardened service start.
systemctl unmask coturn.service
systemctl enable --now coturn
systemctl is-active --quiet coturn
systemctl show coturn -p User -p Group -p Requires -p BindsTo -p After
python3 -B tools/deploy/minaweb/check-turn-egress.py --ip VPS_PUBLIC_IPV4
python3 -B tools/deploy/minaweb/check-turn-egress.py --ip VPS_PUBLIC_IPV4 \
  --target listener
nft list table inet mina_turn
# Expected: allowed delivery plus a drop-counter increase of at least two.
# The test used and closed UDP 40000 and 39999; now start the native relay.
systemctl start mina-relay
systemctl is-active --quiet mina-relay
runuser -u mina -- python3 -c \
  'import json; assert json.load(open("/etc/mina-relay/ice-servers.json")) == []'
```

The local TURN probe requires authenticated allocation, confirms delivery to a
receiver on a WebRTC port, and rejects delivery to both an unrelated UDP
receiver on the same allowed IP and TURN's own STUN listener. A negative-only
check is insufficient. It deallocates before exiting. For repeat tests with the
node running, choose an unused WebRTC port with `--allowed-port`; never stop or
reuse another service's socket. Also run the external forced-TURN sync gate.

The guard loads before coturn at boot and is reinstalled before every coturn
restart. Stopping the guard unit stops coturn through `BindsTo`; its rules stay
installed. After changing the port ranges, stop coturn, update `relay.env`, the
TURN config and incoming firewall together, reload `mina-turn-egress`, inspect
its rules and rerun the probe before restarting the node. A failed atomic nft
update preserves the previous rules and prevents coturn startup. Never flush
nftables or delete this table while coturn runs. After reboot, verify both
units, the rules and the probe again.

Keep the provider console open during firewall setup. Permit TCP 22 only from
the owner, TCP 80/443 for ACME/HTTPS, TCP 8302 for libp2p, UDP 40000–40999 for
WebRTC, and UDP/TCP 3478 for TURN/STUN. The shared firewall template also opens
UDP 443 for its Caddy alternative; nginx does not require that rule. Keep
3000–3002 and TURN allocation ports 49152–49999 closed to external clients.
TURN-to-relay traffic stays on this host; do not copy these firewall assumptions
to a separate TURN server. Schedule certbot renewal with a successful-renewal
hook that runs `nginx -t && systemctl reload nginx`; test
`certbot renew --dry-run`.

nginx permits six signalling requests and six credential requests per minute per
source IP, with a burst of ten and eight concurrent requests. Excess requests
receive 429. Shared NAT users share those budgets. Tune from observed launch
traffic; do not silently remove the limits. These are application limits, not
DDoS protection. No visitor offer URLs are logged; nginx request error logging
is disabled too. Monitor service health separately.

## R2 and Pages headers

Set the bucket CORS policy with
`npx wrangler r2 bucket cors set minaweb-assets --file tools/deploy/minaweb/r2-cors.json`.
This is Wrangler's `rules`/`allowed` format, permitting only
`https://minaweb.org`. The dashboard uses a different format; do not paste this
file there. See the
[R2 command reference](https://developers.cloudflare.com/r2/reference/wrangler-commands/).

On hostname `assets.minaweb.org`, create response-header Transform Rules that
set `Access-Control-Allow-Origin: https://minaweb.org` and
`Cross-Origin-Resource-Policy: cross-origin` on every response, including cached
responses originally requested without an Origin header. Set
`Cache-Control: public, max-age=31536000, immutable` only for successful
`/releases/` objects. Ensure `.wasm` has `Content-Type: application/wasm`,
`.postcard`/`.bin` have `application/octet-stream`, and `.json` has
`application/json`. Never set Content-Encoding on raw uploaded objects. Rclone
infers file types; a rule for the `.wasm` suffix removes dependency on the
upload machine's MIME table. Optional CDN compression is outside the baseline
test; integrity is checked on decoded bytes.

The generated Pages `_headers` sets COOP `same-origin`, COEP `require-corp`,
CORP `same-origin`, nosniff and cache revalidation. Pages serves normal HTML,
CSS, JavaScript and JSON MIME types. The WASM and circuits stay off Pages; the
recipe rejects any Pages file over 25 MiB. The browser checks the WASM's SHA-256
before compilation, and the WASM checks compiled circuit pins.

## First build and deploy

Check out the intended commit in a clean build worktree. Install the versions
listed in the header of `tools/regression/webnode/build.sh`. A matching native
devnet node must have populated the two verifier cache files in the build user's
circuit cache; the script validates their source digests and refuses a release
without caches. Set `PROVER_CIRCUITS` to a complete OCaml devnet dump matching
`tools/mesa-circuits/devnet-blobs.sha256`. The release includes these files for
the browser's block producer and SNARK worker. Changed, missing or unknown
prover files fail the build. Both same-origin and external circuit hosting
require canonical prover hashes plus build pins before decoding. A role's
start-up waits for its isolated prover and shows an integrity error if the
prover's assets cannot be verified.

```bash
# Store and edit this copy outside the checkout; it holds public values only.
cp tools/deploy/minaweb/release.env.example /secure/config/minaweb-release.env
set -a
. /secure/config/minaweb-release.env
set +a
nice -n 10 env CARGO_BUILD_JOBS=8 tools/regression/lock.sh build -- \
  python3 -B tools/deploy/minaweb/release.py COMMIT /srv/releases/NEW_RELEASE
```

The script builds only; it prints no cloud credentials. Test locally as below,
then review `manifest.json` and `deploy.sh`. Run the generated rclone upload and
download-verification commands first. They preserve object paths and reject
replacement. Inspect live asset headers both with and without an Origin header,
and download the WASM to verify the manifest hash. Then run the generated
`wrangler pages deploy` command, or set `ASSET_HEADERS_VERIFIED=yes` and run
`bash deploy.sh` from the retained release. Do not mark headers verified until
the actual R2 custom domain passes. The deploy script contains an exact project,
branch, commit and R2 release prefix, and checks local SHA256SUMS first.

The uploaded page is visible immediately at the project's Pages hostname; attach
the canonical custom domain before accepting the browser launch gate. Cloudflare
account IDs, DNS, payment method and credentials are owner inputs.

## Launch check from outside the VPS

From another network, open an SSH tunnel for the private API, for example
`ssh -N -L 13000:127.0.0.1:3000 OWNER@relay.minaweb.org`. Keep that tunnel
private. Install the browser harness prerequisites, then:

```bash
RELAY_HTTP=http://127.0.0.1:13000 CHROME=/opt/chrome/chrome \
  PUPPETEER_DIR=/opt/browser-harness \
  python3 -B tools/deploy/minaweb/check.py /srv/evidence/launch \
  --turn-ip VPS_PUBLIC_IPV4
```

The gate checks COOP/COEP and WASM plus both verifier caches' CORS/CORP/MIME,
with and without an Origin header. It downloads both caches and checks their
source-digest prefixes against the checkout used to run the gate; use the
release's source commit. It checks HTTPS signalling, closed operator routes,
no-store credential delivery with a username expiry 1 minute to 24 hours ahead
(60 seconds clock tolerance), a real STUN binding response and synced native
devnet peers. It then runs the locked `run.mjs` harness twice with fresh
browsers: normal ICE and forced TURN. Both must reach Synced within one block of
the relay with a connected peer; TURN must select a relay candidate. A PASS file
is written only after all gates pass. Add `--automated-only` to stop after the
HTTP, credentials, STUN and native-peer checks: this writes `AUTOMATED-PASS`,
never `PASS`, and leaves both browser sync runs outstanding. The timestamp check
is not an HMAC verification; the forced-TURN run demonstrates the server accepts
the issued credentials. A browser reaching Synced alone is not proof of cache
delivery because it can rebuild missing verifier indexes. A malformed signalling
offer alone is not connectivity evidence. Keep a second real-browser check from
a different ISP and a UDP-restricted network on the owner launch checklist.

## Local two-origin test

Build with `RELAY_DOMAIN=relay.minaweb.test` and
`ASSETS_ORIGIN=https://assets.minaweb.test`, using an owned devnet relay's peer
ID. Use a transient user unit for that relay, HTTP 4271, signalling 4272, libp2p
9400 and WebRTC UDP 9401–9409. Do not use an occupied port or a shared node.
Record the native binary's build-info and SHA-256 before the test.

`tools/deploy/minaweb/local.Caddyfile` serves the page on 4274, assets on 4275
and HTTPS signalling on 4276, all loopback only. Set `RELEASE_DIR` to the
release directory and `RELEASE_PREFIX` to its manifest's `release` field. Run
Caddy as an unprivileged owned process with private XDG data/config directories.
It creates a private CA without installing system trust. Obtain its
root/intermediate SPKI hashes with OpenSSL. Set `CA_DIR` to the private data
directory's `caddy/pki/authorities/local`, then run:

```bash
spki() {
  openssl x509 -in "$1" -pubkey -noout |
    openssl pkey -pubin -outform DER |
    openssl dgst -sha256 -binary | openssl base64 -A
}
SPKI_HASHES="$(spki "$CA_DIR/root.crt"),$(spki "$CA_DIR/intermediate.crt")"
```

Start the browser with those certificate pins:

```bash
BASE=https://minaweb.test PAGE=/ AUTOSTART=1 EVERY_MS=5000 UNTIL_SYNCED=30 \
  CHROME_ARGS="--host-resolver-rules=MAP minaweb.test 127.0.0.1:4274,MAP assets.minaweb.test 127.0.0.1:4275,MAP relay.minaweb.test 127.0.0.1:4276 --ignore-certificate-errors-spki-list=$SPKI_HASHES" \
  OUT=/srv/evidence/local-browser node tools/regression/webnode/run.mjs 15
```

Set CHROME and PUPPETEER_DIR as above. Check `crossOriginIsolated=true`, remote
WASM and circuit HTTP 200 responses, Synced above genesis with a peer, and a tip
within one block of the owned relay. The harness's process exit alone is not a
sync assertion. This loopback fixture returns `[]` for ICE; it qualifies the
split asset path and direct signalling, not public NAT or TURN. The external
launch gate remains required. Stop every owned process by its PID, wait for the
user unit to stop, and remove only this build's Cargo target.

## Routine release and rollback

For each release, record the full source commit and relay binary hash. Run the
same clean build recipe into a new directory, local checks, immutable upload,
header check and Pages deploy. Record the Pages deployment ID and manifest
SHA-256. Keep at least the last two working bundles and their R2 namespaces. Do
not mutate the peer list in place: release a new Pages bundle so its manifest
remains accurate. Credentials remain fresh on every boot because they come from
the issuer, including for old deployments.

Rollback Pages by selecting the previous production deployment in the dashboard,
or deploying its retained `browser/` directory with the generated Pages command.
Keep its R2 files. Do not rebuild an old commit and assume identical bytes or
settings. Re-run the external check after rollback.

For relay updates, install and verify a new directory under
`/opt/mina-relay/releases/`, retaining the old directory, identity and state.
Check its CLI opt-in flag and full commit before swapping `current` atomically:

```bash
ln -s releases/NEW_COMMIT /opt/mina-relay/current.next
mv -Tf /opt/mina-relay/current.next /opt/mina-relay/current
systemctl restart mina-relay
```

Wait for readyz and native peers, then run the browser gate. Roll back the
symlink the same way if needed. Inspect persistence compatibility notes before
downgrading; back up the stopped state first, and permit a fresh bootstrap when
the older build cannot restore it. Do not delete identity or change the peer ID
as a troubleshooting shortcut. Restart the credential service too if its code
changed. Compare templates with installed configuration without exposing
secrets.

## Budget and monitoring

Historical measurements found 134–136 MiB relay-to-browser traffic for a cold
devnet bootstrap: about 141–143 GB per 1,000 new visitors, or 1.41–1.43 TB per
10,000, before retries, native gossip and transport overhead. The measured
mainnet comparison was 241.8–241.9 MiB, about 254 GB per 1,000; do not budget
devnet using mainnet numbers. Short devnet following windows used roughly
1.3–1.5 MiB/minute, but are not representative monthly capacity measurements.
Browser memory is substantial: roughly a GiB of WASM plus browser overhead.

The historical static page was 44.6 MiB uncompressed (12.1 MiB with only WASM
brotli-compressed). Use the new manifest for this release's actual static bytes.
Those downloads use R2/Pages, while ledger bootstrap uses VPS traffic. R2's
Internet egress is free; storage and request operations above its free
allowances are not. Standard rates at preparation time are $0.015/GB-month,
$4.50/million Class A and $0.36/million Class B operations, with monthly free
allowances of 10 GB-month, 1 million A and 10 million B. Recheck
[R2 pricing](https://developers.cloudflare.com/r2/pricing/) before purchase.
Domain renewal, VPS rental/outbound overage, backups and any paid Cloudflare
features remain separate. Budget the VPS from its actual quote; for example a
$20/month VPS with 2 TB included is an assumption, not a provider offer.

Budget relay egress as cold visits times bootstrap bytes, plus following minutes
times a measured active rate, plus retries, native gossip and transport
overhead. TURN on the same host adds CPU and overhead but no second external hop
to the relay. Ten thousand cold devnet visits can consume most of a 2 TB
allowance before following traffic. Add billing alerts at 50/75/90 percent and a
documented owner decision to reduce admission or disable new sessions before
overage.

Monitor HTTPS healthz and readyz every minute from outside, certificate expiry,
native connected peers and tip progress via the private API, systemd restart
counts, memory/disk, TURN allocation counts and VPS outbound bytes. A quiet
chain can have an old tip; compare a separate devnet source before declaring
sync failure. Run a scheduled real-browser smoke at a budgeted cadence: it
consumes bootstrap bandwidth. Avoid storing offer URLs, ICE credentials or
visitor IPs in public monitoring. Keep operational logs short and private.

## Launch checklist

- [ ] Agent: signed preparation commits, manifest, local split-origin sync
      evidence and export inventory pass; independent review by coordinator.
- [ ] Owner: Cloudflare account/MFA, zone delegation, R2 billing, Direct Upload
      project, bucket-scoped credentials and private rclone configuration ready.
- [ ] Owner: VPS/public IPv4 purchased, DNS-only relay record resolves, budget
      alerts configured, SSH/console access tested.
- [ ] Owner: qualified post-173 relay binary/hash, stable peer ID, private
      identity backup, native peers and readyz verified.
- [ ] Owner: nginx certificate/renewal, firewall, TURN secret, issuer and quotas
      installed; operator routes return 404 externally.
- [ ] Agent: build from the approved commit with the owner's public values;
      review manifest and exact deployment commands.
- [ ] Owner: R2 immutable upload and download verification; CORS/CORP/MIME
      correct with and without Origin; assets readable before Pages deploy.
- [ ] Owner: Pages deployment ID recorded; canonical domain attached;
      crossOriginIsolated true in a fresh browser.
- [ ] Agent: external `check.py` PASS, direct and forced-TURN synced tips; owner
      checks another ISP and a restricted network.
- [ ] Owner: monitoring, cost alerts, rollback bundle and prior R2 namespace
      retained; short-TTL renewal and issuer-outage behavior checked.
- [ ] Owner: approve launch announcement only after all gates pass.
