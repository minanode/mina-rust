# Minaweb devnet launch

This is a preparation kit. No account creation, DNS change, upload, purchase or
deployment is performed by the build recipe. The owner runs the production
commands after review. The initial app is the view-only minaweb UI on devnet.

## Release contract

Use a clean, reviewed commit containing the exact seed-relay allowlist, the
disabled browser discovery and signalling channels, and the WASM integrity
check, all in 0.20.1 and later. The page reaches the relay through the relay's
direct HTTPS signalling listener. The relay introduces peers to each other only
in browser sharing's limited relay mode, which is off unless the owner turns it
on; see [Browser sharing](#browser-sharing).

The released **0.20.0 binary is not suitable for a public relay**: it predates
the seed-relay allowlist. Use 0.20.1 or later, or a reviewed main commit built
with `tools/deploy/relay/build-relay.sh`. Record the binary's full embedded
commit, SHA-256 and CLI help, not the version string alone. `start-relay.sh`
rejects 0.20.0, which has neither `--webrtc-signaling-relay` nor
`--webrtc-sharing-relay`. Stock 0.20.0 remains useful for historical
comparisons, not launch acceptance.

The bundle contains `browser/`, `upload/`, `manifest.json`, `SHA256SUMS` and
`deploy.sh`. The manifest records the source commit, build settings, every
served file's size and SHA-256. Asset keys are
`releases/FULL_COMMIT/RANDOM_RELEASE_ID/wasm/...` and
`releases/FULL_COMMIT/RANDOM_RELEASE_ID/circuit-blobs/...`. Each build gets a
new namespace, including rebuilds of one commit with different settings. Never
overwrite or delete a release that a deployed page or rollback needs. Rclone's
immutable mode refuses changed remote objects; deploy one release at a time.
Retain the whole local bundle with its manifest.

## Relays

minaweb.org runs two relays on Hetzner Cloud, in two EU locations:

| Relay                | Location           | DNS                           |
| -------------------- | ------------------ | ----------------------------- |
| `relay1.minaweb.org` | Falkenstein (fsn1) | DNS-only A record to its IPv4 |
| `relay2.minaweb.org` | Helsinki (hel1)    | DNS-only A record to its IPv4 |

The page lists every relay, and each web node dials all of them. It keeps
following the chain while one relay is down. The seed allowlist holds exactly
the listed relays.

Each host stands alone: its own libp2p identity and peer ID, TURN secret,
credential service, coturn, firewall, certificate and peer limit. Hosts share no
secret. Each coturn forwards only to the relay on its own host.

The page fetches an ICE server list from each relay's credential service,
`https://RELAY/ice-servers.json`, and hands the node all of them. It renews each
list before its credentials expire, on that list's own schedule. An issuer that
is down delays only its own relay's TURN. Its expired credentials are left out
of the node's list. While a relay's host is down, its unreachable TURN stays in
the list until its credentials expire. The node waits at most 3 s for ICE
gathering, so each new connection can take up to 3 s longer then; open
connections are unaffected.

One relay is a valid configuration: a build with one `RELAYS` entry. Adding a
relay changes nothing on the existing hosts. Install the new host, then build
and deploy a page that lists every relay. A further relay, such as one in the
US, is one more host and one more entry.

### relay1, installed from the single-relay kit

`relay1.minaweb.org` runs in fsn1, installed from the single-relay version of
this runbook. Its peer ID is
`12D3KooWC4k4X9Ub9JBbLWov3UCH1q4MH7iVEZM2dVfx1dhnYSMo`. It needs no reinstall.
Bring it up to this kit with
[Update an installed relay to this kit](#update-an-installed-relay-to-this-kit).
The page built from this kit fetches its ICE servers from
`https://relay1.minaweb.org/ice-servers.json` directly, not through a Pages
redirect.

### relay2, installed from this kit

`relay2.minaweb.org` runs in hel1, installed on 2026-11-02 from this runbook on
a fresh host, with browser sharing on. Its peer ID is
`12D3KooWN3uRLCr1uZHCVQ11CdvYdXTsQinGQ8JDMah3MZy8M8Q7`. It reboots for updates
at 05:00 UTC.

## Browser sharing

Visitors can turn on sharing on the page's Peers page (off by default, per
browser). A sharing page still dials only the listed relays. It asks them to
introduce it to other sharing pages, and connects to at most 4 of them. Without
a relay in sharing mode, a sharing page finds no browsers and nothing else
changes. Each sharing page enforces two things itself, whatever the relay or the
other page does:

- It serves bootstrap data (snarked ledger queries, the staged ledger, blocks)
  to at most 2 browsers at once, counted from the requests it serves, not from
  what a browser claims. A third browser's requests get an empty answer, and its
  node fetches that data from a relay instead.
- It lets its browser run connectivity checks only to an introduced browser's
  public addresses and random mDNS names (`<uuid>.local`), at ports of 1024 or
  more, at most 12 of them. Private, loopback, link-local, carrier-NAT,
  multicast and special-purpose addresses are dropped, so a modified page can't
  aim a visitor's browser at an address on the visitor's network. It can still
  name any public address on a high port, and the browser sends connectivity
  checks there: up to 12 candidates per introduction, within the introduction
  limits. A peer that sends its private address instead of an mDNS name reaches
  another browser only over a public address or TURN.

`--webrtc-sharing-relay` is that mode. In it the relay:

- introduces only pages that opened the signalling channels and asked; a page
  with sharing off never opens them, and libp2p peers never have them;
- counts introductions over an hour: at most 6 involving one peer key, 12
  involving one address (an IPv4 address or an IPv6 /64) and 120 in all;
- never repeats a pair within the hour, hands each request the sharing page it
  introduced least, and tries not to pair two pages that are both still syncing
  (more than 5 blocks below its tip, or fetching bootstrap data from the relay
  in the last 30 s). That is best effort: a page can claim a tip it doesn't
  hold. The limits above, and each page's own serving cap, are what protect
  visitors;
- tries a request that waited at a limit again every 10 s;
- asks no one for introductions and takes no offers itself; libp2p and OCaml
  peering is unchanged;
- logs each introduction (`P2pSharingIntroduction`) and each request left
  waiting at a limit (`P2pSharingIntroductionRefused`) at info, with peer IDs
  and no addresses, and counts both in `mina_webrtc_sharing_introductions_total`
  and `mina_webrtc_sharing_introductions_refused_total`.

The per-address limit needs the visitor's address. The relay takes it from
`X-Forwarded-For` only when the request comes from a proxy on the same host
(loopback), using the last address the proxy added. `relay.nginx.conf` sets the
header from `$remote_addr`; Caddy sets it by default. An include from before
0.21.0 does not set it, so nginx passes on whatever `X-Forwarded-For` the
visitor sent, and a visitor can claim any address. Install the new include
before turning sharing on. The relay keeps an address only while that peer is
connected, at most 1024 of them, and only in sharing mode; no log or request
record holds it.

The relay trusts that header from every local process, not only from nginx. A
process on the relay host can send offers with any `X-Forwarded-For` and so
escape the per-address limit. The per-key and total limits still hold. Run
nothing else on a relay host. The node's HTTP port, 3000, is on loopback too,
with its whole API and no authentication, so a process on the host can do more
there than through this header. Narrowing the header's trust alone would not
change what such a process can do, so the kit does not.

Even within the limits, a modified page with sharing on can collect the
addresses of the pages it is introduced to: about 6 an hour per key, 12 an hour
per address, per relay. An attacker with many addresses, such as an IPv6 /48 or
rented hosts, is held only by the total, and each sharing page is handed to at
most 6 requesters an hour. Such an attacker can also use up the 120 an hour and
starve honest introductions for that hour. The page and the docs say so before a
visitor turns sharing on.

Each sharing connection between two browsers costs the relay nothing beyond the
two encrypted messages it forwards.

### Turn sharing on for a relay

Turning sharing on is an owner decision, made after the independent security
review. Install all four parts together, in this order. A binary with the flag
behind an older nginx include trusts the visitor's own `X-Forwarded-For`, so a
visitor could claim any address for the per-address limit.

1. Install 0.21.0 or later, or a release built from main (`build-relay.sh`),
   with the `start-relay.sh` from the same commit, as under
   [Routine release and rollback](#routine-release-and-rollback). Do not switch
   `current` yet. Check that the new binary has the flag:

   ```bash
   /opt/mina-relay/releases/NEW_COMMIT/mina node --help |
     grep -o -- --webrtc-sharing-relay
   ```

2. Install this kit's `relay.nginx.conf` for the relay's name (as under
   [TLS and firewall](#tls-and-firewall)), check and reload nginx. Its
   signalling location sets `X-Forwarded-For` from `$remote_addr`, replacing any
   value the visitor sent:

   ```bash
   sed "s/<RELAY_DOMAIN>/$RELAY_DOMAIN/g" \
     tools/deploy/minaweb/relay.nginx.conf >/etc/nginx/conf.d/minaweb.conf
   nginx -t && systemctl reload nginx
   grep -c 'X-Forwarded-For $remote_addr' /etc/nginx/conf.d/minaweb.conf
   ```

   The count is 2: the signalling and credential locations.

3. Set exactly one `SHARING_RELAY=on` line in `relay.env`. A relay installed
   before 0.21.0 has no such line, and a bare `sed` substitution then changes
   nothing, so remove any old line and append the new one:

   ```bash
   sed -i '/^SHARING_RELAY=/d' /etc/mina-relay/relay.env
   echo SHARING_RELAY=on >>/etc/mina-relay/relay.env
   grep -c '^SHARING_RELAY=on$' /etc/mina-relay/relay.env
   grep '^EXTRA_ARGS=$' /etc/mina-relay/relay.env
   ```

4. Switch `current` to the new release and restart the relay. Check that it runs
   with the flag:

   ```bash
   ln -s releases/NEW_COMMIT /opt/mina-relay/current.next
   mv -Tf /opt/mina-relay/current.next /opt/mina-relay/current
   systemctl restart mina-relay
   # The main process is start-relay.sh until it execs the node, and systemd
   # gives a restarted service a new PID: read it again each second, 60 s at
   # most.
   for _ in $(seq 60); do
     pid=$(systemctl show -p MainPID --value mina-relay)
     [[ $(readlink /proc/"$pid"/exe 2>/dev/null) == */mina ]] && break
     sleep 1
   done
   [[ $(readlink /proc/"$pid"/exe 2>/dev/null) == */mina ]] ||
     { echo 'mina-relay did not start: journalctl -u mina-relay' >&2; false; }
   args=$(tr '\0' ' ' </proc/"$pid"/cmdline)
   grep -o -- --webrtc-sharing-relay <<<"$args"
   ```

`start-relay.sh` refuses `SHARING_RELAY=on` with a binary that lacks the flag,
and refuses the flag in `EXTRA_ARGS`, so `relay.env`'s one line is the switch.
To turn sharing off, set `SHARING_RELAY=off` the same way and restart the relay.
Keep the nginx include: it is the safe one either way.

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
   [Cloudflare's rclone setup][r2-rclone].
3. Authenticate Wrangler with `npx wrangler login`, or inject a narrowly scoped
   account Pages token from a secret manager. Record the installed Wrangler and
   rclone versions with the release receipt.
4. Provision one Hetzner Cloud server per relay, in its location (see
   [Provisioning](#provisioning)): Ubuntu 24.04, x86-64 with AVX2, a public
   IPv4, initially 4 vCPU, 8 GiB RAM and 40 GB disk. Hetzner includes 20 TB of
   outbound traffic per server; set billing alerts. These are starting
   allocations, not measured capacity guarantees. Create each relay's name as a
   **DNS-only** A record to its server. Do not publish AAAA records until IPv6
   works end to end.
5. Install each relay and its credentials service below; record each public peer
   ID. The owner keeps each identity backup and TURN secret privately.
6. Configure the R2 and Pages headers below, then build, test, upload assets,
   deploy Pages and attach `minaweb.org` in Pages Custom Domains. Redirect `www`
   and the Pages project hostname to the canonical HTTPS origin. Test production
   on `minaweb.org`: preview hostnames are intentionally not on the
   asset/credential CORS allowlist.

### Set up Cloudflare through the API

The Cloudflare parts of steps 1, 2, 3 and 6 can run through the Cloudflare API
instead of the dashboard. minaweb.org was set up this way on 2026-11-02. The
calls need a token with these permissions:

- account: Cloudflare Pages Edit and Workers R2 Storage Edit;
- zone `minaweb.org`: DNS Edit, Transform Rules Edit, and Single Redirect
  (Dynamic URL Redirects) Edit.

Grant the account permissions on the account that hosts minaweb.org only, not on
all accounts. Workers R2 Storage Edit covers every bucket in the account, and
provisioning needs it to create the bucket.

Pass the token to `curl` on stdin, as under [Provisioning](#provisioning). Each
call below is relative to `https://api.cloudflare.com/client/v4`:

1. Create the Direct Upload project. A project created without a `source` is a
   Direct Upload project. `POST /accounts/ACCOUNT_ID/pages/projects` with
   `{"name":"minaweb","production_branch":"main"}`.
2. Create the bucket with `POST /accounts/ACCOUNT_ID/r2/buckets` and
   `{"name":"minaweb-assets","storageClass":"Standard"}`. Without a
   `locationHint`, R2 places the bucket near the caller. `minaweb-assets` is in
   WNAM (western North America). Its `r2.dev` endpoint is off by default:
   `GET .../r2/buckets/minaweb-assets/domains/managed` shows `"enabled":false`.
3. Set CORS with `PUT .../r2/buckets/minaweb-assets/cors` and
   `tools/deploy/minaweb/r2-cors.json` as the body. The API takes Wrangler's
   format unchanged.
4. Attach the asset host with
   `POST .../r2/buckets/minaweb-assets/domains/custom` and this body:

   ```json
   {
     "domain": "assets.minaweb.org",
     "zoneId": "ZONE_ID",
     "enabled": true,
     "minTLS": "1.2"
   }
   ```

   R2 creates the proxied DNS record itself. Its certificate was active within
   five minutes.

5. Attach the site with `POST .../pages/projects/minaweb/domains` and
   `{"name":"minaweb.org"}`. This call creates no DNS record. Create a proxied
   `CNAME` from `minaweb.org` to `minaweb.pages.dev` with
   `POST /zones/ZONE_ID/dns_records`. The domain becomes active once the record
   resolves.
6. For `www`, create a proxied `AAAA` record for `www.minaweb.org` to `100::`.
   Then load `tools/deploy/minaweb/cloudflare-redirects.json` with:

   ```text
   PUT /zones/ZONE_ID/rulesets/phases/http_request_dynamic_redirect/entrypoint
   ```

   The rule sends a 301 to `https://minaweb.org` and keeps the path and the
   query.

7. Load the asset headers of [R2 and Pages headers](#r2-and-pages-headers),
   `tools/deploy/minaweb/cloudflare-asset-headers.json`, with:

   ```text
   PUT /zones/ZONE_ID/rulesets/phases/http_response_headers_transform/entrypoint
   ```

A `PUT` to a phase entrypoint replaces every rule in that phase. Read the phase
first with `GET` on the same path. If it already holds other rules, add each new
rule with `POST /zones/ZONE_ID/rulesets/RULESET_ID/rules` instead.

R2's S3 API accepts the same token. The access key ID is the token's ID, from
`GET /user/tokens/verify`. The secret access key is the SHA-256 hex digest of
the token value. rclone reads a remote from its environment, so no
`rclone config` file is needed: set `RCLONE_CONFIG_R2_TYPE=s3`,
`RCLONE_CONFIG_R2_PROVIDER=Cloudflare`, `RCLONE_CONFIG_R2_REGION=auto`,
`RCLONE_CONFIG_R2_ENDPOINT`, `RCLONE_CONFIG_R2_ACCESS_KEY_ID` and
`RCLONE_CONFIG_R2_SECRET_ACCESS_KEY` in the upload shell only. Wrangler reads
`CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`.

For routine uploads, use a separate R2 token with **Object Read & Write** scoped
to the `minaweb-assets` bucket, as in step 2 of
[Owner setup, in order](#owner-setup-in-order). A leaked upload token then
reaches one bucket's objects. The provisioning token can create and delete every
bucket in the account.

Four settings need more than these permissions. Set them in the dashboard:

- **The Pages hostname.** `minaweb.pages.dev` is not in the `minaweb.org` zone,
  so a zone redirect rule never sees it. Redirect it with an account-level Bulk
  Redirect. Under **Bulk Redirects**, create a list with one URL redirect:

  | Setting               | Value                 |
  | --------------------- | --------------------- |
  | Source URL            | `minaweb.pages.dev`   |
  | Target URL            | `https://minaweb.org` |
  | Status                | 301                   |
  | Preserve query string | On                    |
  | Subpath matching      | On                    |
  | Preserve path suffix  | On                    |
  | Include subdomains    | Off                   |

  The list alone redirects nothing, and Cloudflare shows it as **Inactive**.
  Then create a Bulk Redirect Rule that uses the list. With subdomains off,
  preview hostnames (`HASH.minaweb.pages.dev`) still serve each deployment.

- **Browser Cache TTL.** On the `minaweb.org` zone, set **Caching >
  Configuration > Browser Cache TTL** to **Respect Existing Headers** before the
  second deployment. The zone default of four hours overrides the page's
  `no-cache`. With it, `minaweb.org` sends `Cache-Control: max-age=14400` for
  static file types, such as `.js` and `.svg`. `minaweb.pages.dev` is not
  affected. The WASM glue, `assets/webnode/pkg/mina_node_web.js`, has no hash in
  its name. After a new deployment, a returning browser can then pair its cached
  glue with the new WASM for up to four hours.
- **Web Analytics.** Cloudflare adds its Web Analytics beacon
  (`static.cloudflareinsights.com/beacon.min.js`) to every HTML response on
  `minaweb.org`. The page's Content-Security-Policy refuses it, so it sends
  nothing, and each load logs a CSP error. Turning off RUM under **Analytics &
  Logs > Web Analytics** does not stop the injection. Create a Configuration
  Rule on the zone instead. It matches hostname equals `minaweb.org` and sets
  **Disable Real User Monitoring (RUM)** to On. To check, send a browser-like
  request and look for the beacon. The command prints nothing once the rule
  works:

  ```bash
  UA='Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko)'
  curl -s -H 'Accept: text/html' -A "$UA Chrome/130.0 Safari/537.36" \
    https://minaweb.org/ | grep beacon.min.js
  ```

- **Edge caching of the release assets.** Cloudflare does not cache `.wasm`,
  `.postcard` and `.json` by default (`cf-cache-status: DYNAMIC`), so every
  visitor reads the WASM and the verifier caches from the bucket. The
  response-header Transform Rule sets browser caching, not edge caching. Create
  a Cache Rule on the zone:
  - **Match**: Hostname equals `assets.minaweb.org` AND URI Path starts with
    `/releases/`.
  - **Cache eligibility**: Eligible for cache.
  - **Edge TTL**: Use cache-control header if present, use default Cloudflare
    caching behavior if not.
  - **Browser TTL**: Respect origin.

  The expression form of the match is:

  ```text
  (http.host eq "assets.minaweb.org"
    and starts_with(http.request.uri.path, "/releases/"))
  ```

  The release objects carry `public, max-age=31536000, immutable` from upload,
  and each release has its own prefix, so a cached object never goes stale. To
  check, fetch the WASM twice. The second response has `cf-cache-status: HIT`.
  R2 egress is free either way. The rule saves R2 read operations.

Cloudflare's Browser Integrity Check refuses urllib's default User-Agent with
403 (error code 1010) on Pages and on the asset host. `check.py` sends its own
User-Agent.

## Provisioning

The coordinator provisions and manages the hosts with the `hcloud` CLI and the
Cloudflare API. Both tokens come through a secret request: a project-scoped
Hetzner Cloud API token, and a Cloudflare token limited to DNS edit on
`minaweb.org`. They live only in the provisioning shell's environment
(`HCLOUD_TOKEN`, `CF_API_TOKEN`), never in chat, files, shell history or this
repository. This kit calls no provider API itself.

Per relay, with `NAME` (`relay1`), `LOCATION` (`fsn1`), a server `TYPE` with 4
vCPU and 8 GB (`hcloud server-type list`) and the owner's SSH key name:

```bash
hcloud server create --name "$NAME" --location "$LOCATION" \
  --type "$TYPE" --image ubuntu-24.04 --ssh-key "$SSH_KEY" \
  --without-ipv6 --firewall minaweb-relay \
  --label role=relay --label network=devnet \
  --user-data-from-file tools/deploy/minaweb/cloud-init.yaml
hcloud server enable-protection "$NAME" delete rebuild
IPV4=$(hcloud server ip "$NAME")
RECORD="{\"type\":\"A\",\"name\":\"$NAME.minaweb.org\",\"content\":\"$IPV4\","
RECORD="$RECORD\"proxied\":false,\"ttl\":300}"
# The token goes to curl on stdin, not in its arguments.
printf 'header = "Authorization: Bearer %s"\n' "$CF_API_TOKEN" |
  curl -fsS -K - -X POST -H 'Content-Type: application/json' \
    "https://api.cloudflare.com/client/v4/zones/$ZONE_ID/dns_records" \
    --data "$RECORD"
```

Both relays are `cpx32` servers. They have no IPv6 address, so there is no AAAA
record to leave out. The Hetzner Cloud Firewall `minaweb-relay` holds the
inbound rules of [TLS and firewall](#tls-and-firewall), without ufw's UDP 443,
and every relay shares it. Protection stops a stray `hcloud server delete` or
`rebuild`.

`cloud-init.yaml` masks `coturn.service`, installs every pending update and
reboots if one needs it. It turns on automatic security updates with reboots at
04:00 UTC (see [Updates](#updates)), installs the packages, and disables the
distribution's nftables unit. It does not install Ubuntu's coturn package. It
holds no secret.

On the new host, check that the first boot finished its upgrade and that the CPU
has AVX2:

```bash
cloud-init status --wait
! test -e /run/reboot-required
apt list --upgradable 2>/dev/null | grep -c upgradable
grep -qw avx2 /proc/cpuinfo
```

The count is 0. On every relay after the first, give the automatic reboot its
own hour, so that two relays never reboot together:

```bash
sed -i 's/"04:00"/"05:00"/' /etc/apt/apt.conf.d/52mina-relay
```

Then run [Relay installation](#relay-installation) on it; every step there is a
non-interactive command for one host.

## Relay installation

Repeat this section on each relay host. Set the host's two values first; the
commands below use them:

```bash
RELAY_DOMAIN=relay1.minaweb.org # relay2.minaweb.org on the second host
VPS_IPV4=203.0.113.10           # this host's public IPv4
```

Transfer the kit directories to an owner-controlled VPS staging directory,
preserving `tools/deploy/relay` and `tools/deploy/minaweb`, and run the
installation commands there. The commands in this section run as root on the
**new VPS only**, targeting Ubuntu 24.04 and coturn 4.18.0 from
[Build coturn](#build-coturn). Never install Ubuntu's `coturn` package: its
4.6.1 acts on unsigned attributes after MESSAGE-INTEGRITY (CVE-2026-68554), and
its defaults can start an anonymous TURN listener. Keep `coturn.service` masked
until the kit's unit replaces it. `cloud-init.yaml` has done the first block
already:

```bash
systemctl mask coturn.service
apt-get update
apt-get -y full-upgrade
apt-get install nginx certbot ufw nftables python3 unattended-upgrades \
  libevent-core-2.1-7t64 libevent-extra-2.1-7t64 libevent-openssl-2.1-7t64 \
  libevent-pthreads-2.1-7t64 libmicrohttpd12t64 libssl3t64
# Before installing any firewall rules, disable the generic flush-on-stop unit.
systemctl disable --now nftables.service
test "$(systemctl is-enabled coturn.service)" = masked
! dpkg -l coturn 2>/dev/null | grep -q '^ii'
! pgrep -x turnserver
! ss -H -lntu | grep -E ':(3478|5349)[[:space:]]'
```

If coturn is already installed or running, this is not a fresh installation:
stop and investigate its exposure before following this procedure. Keep the mask
through configuration and both firewall installations below. Do not enable the
distribution `nftables.service`: this kit owns a separate table and unit,
without flushing other firewall rules. Do not install Caddy alongside nginx on
ports 80/443. The older Caddy example (`tools/deploy/relay/relay.caddy`) is for
the local test stack and private deployments only. It caps the request body but
has no rate or connection limits. Do not front a public relay with it. This kit
uses nginx's built-in rate limiter.

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

Set `EXTERNAL_IP` in `/etc/mina-relay/relay.env` to the public IPv4:

```bash
sed -i "s/<VPS_PUBLIC_IPV4>/$VPS_IPV4/" /etc/mina-relay/relay.env
```

Keep `NETWORK=devnet`, HTTP 3000 and signalling 3001 on loopback, libp2p 8302,
WebRTC UDP 40000–40999, `MAX_PEERS=100` and `EXTRA_ARGS` empty. The relay's ICE
list is `[]`; the public address makes STUN unnecessary on the server.

Leave `WEBRTC_SCTP_WINDOW` empty for the default of 1 MiB. It is the most data
the relay keeps in flight to one browser, so it bounds how fast a distant
browser downloads: a browser 310 ms away synced in 209 s at 1 MiB and in 276 s
at the old 256 KiB. The setting applies to every WebRTC connection a native node
makes, not only a relay's.

It is not a memory limit. While a browser downloads, its connection's send
buffers hold about the window, plus up to one more message and allocation
overhead. At the default, 100 browsers downloading at once hold a little over
100 MiB in send buffers, plus that overhead, on top of the rest of the relay's
memory.

The relay advertises a WebRTC host candidate for every interface address to
every visitor. A Hetzner Cloud server has only `lo` and `eth0`, so it advertises
its public IPv4 alone; relay1 does. Leave `WEBRTC_NAT_IP` empty there. Check
that `ip -4 -o addr` shows no other non-loopback address. A host with more
interfaces (a private network, a VPN, Docker) would send those addresses too. On
such a host, set `WEBRTC_NAT_IP` to the public IPv4 (`--webrtc-nat-ip`): the
relay then advertises that address in place of its interface addresses.

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

The public peer ID, without showing the key:

```bash
sh -c 'MINA_P2P_SEC_KEY=$(cat /etc/mina-relay/p2p.key) \
  /opt/mina-relay/current/mina keys p2p-key-pair' | sed -n 's/^libp2p_id: *//p'
```

After starting the relay below, its `P2P identity, peer_id` startup record in
`journalctl -u mina-relay` shows the same ID. Save it as this relay's `RELAYS`
entry, `RELAY_DOMAIN=PEER_ID`. Keep the same identity and state directory across
updates. Back up the identity to the owner's secret manager; do not include it
in release bundles.

### Build coturn

The relays run coturn 4.18.0, the latest upstream release, built from its source
tarball by `tools/deploy/minaweb/build-coturn.sh`. Ubuntu 24.04 ships 4.6.1,
which has no fix for CVE-2026-68554 and predates the fixes for 18 later
advisories. 4.18.0 ignores attributes after MESSAGE-INTEGRITY. The script
refuses a tarball whose SHA-256 differs from its pin, builds without any
database backend, and links only Ubuntu's OpenSSL, libevent and libmicrohttpd,
so `apt` updates those libraries. It writes `coturn-4.18.0-noble-amd64.tar`,
with `bin/turnserver` and a `BUILD-INFO` file listing the source hash and build
packages.

Build twice on two throwaway Ubuntu 24.04 x86-64 machines, not on a relay, so
that no compiler is left on a public host. The two tars must have the same
SHA-256:

```bash
sudo apt-get install --no-install-recommends build-essential cmake \
  pkg-config libssl-dev libevent-dev libmicrohttpd-dev
sudo apt-get remove libsqlite3-dev libhiredis-dev # if present
tools/deploy/minaweb/build-coturn.sh out
```

Two builds on 2026-11-01 gave
`3b01c87dc14fbc6b62b963b269a48b5b19537eaf5d7a60d7d8894a6b0a96cb7b`. Copy the tar
to the relay, check its SHA-256 there, and install it with its own user:

```bash
printf '%s  %s\n' \
  3b01c87dc14fbc6b62b963b269a48b5b19537eaf5d7a60d7d8894a6b0a96cb7b \
  coturn-4.18.0-noble-amd64.tar | sha256sum -c
useradd --system --user-group --home-dir / --no-create-home \
  --shell /usr/sbin/nologin turnserver
install -d -m 755 /opt/coturn/4.18.0
tar -C /opt/coturn/4.18.0 --no-same-owner -xf coturn-4.18.0-noble-amd64.tar
ln -sfn 4.18.0 /opt/coturn/current
/opt/coturn/current/bin/turnserver --version
```

coturn is then not an Ubuntu package, so unattended upgrades do not update it.
Watch the [coturn security advisories][coturn-advisories]. For a new release,
update the pin, build, compare and install it into its own
`/opt/coturn/VERSION`. Switch `current`, restart coturn and rerun the TURN probe
under [TLS and firewall](#tls-and-firewall). Keep the previous directory to roll
back.

### Expiring TURN credentials

While coturn is still masked, install its hardened configuration:

```bash
install -o root -g turnserver -m 640 tools/deploy/minaweb/turnserver.conf \
  /etc/turnserver.conf
```

Name the host's own address and relay in it:

```bash
sed -i -e "s/<VPS_PUBLIC_IPV4>/$VPS_IPV4/g" \
  -e "s/<RELAY_DOMAIN>/$RELAY_DOMAIN/" /etc/turnserver.conf
```

Every `<VPS_PUBLIC_IPV4>` becomes the public IPv4 assigned to the host's
interface, so `allowed-peer-ip` is this host's relay only. This kit qualifies
that topology only; the egress guard rejects `external-ip` and differing
listener/relay/allowed addresses. Qualify NAT or a separate TURN server before
changing these assumptions.

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
# TURN_IPV4 is this host's address; turn.env contains no secret.
sed -i "s/^TURN_IPV4=.*/TURN_IPV4=$VPS_IPV4/" /etc/mina-relay/turn.env
install -m 644 tools/deploy/minaweb/mina-turn-credentials.service \
  /etc/systemd/system/
systemctl daemon-reload
! runuser -u mina -- test -r /etc/mina-relay/turn.secret
systemctl enable --now mina-turn-credentials
# coturn remains masked until TLS and both firewalls are ready.
```

The issuer binds only loopback 3002. nginx exposes `/ice-servers.json` over
HTTPS, with no-store and CORS for `https://minaweb.org`. Every page boot fetches
it from each relay. The browser receives a username and an HMAC credential for
that relay's TURN; the shared secret never leaves its VPS. The credential
expires 12 to 24 hours after it is issued. The unauthenticated endpoint is
deliberately public: origin checks are not authentication. Quotas, request
limits and the peer allowlist bound abuse.

Every request from one address (an IPv4 address, or an IPv6 /64) in one 12-hour
window gets the same username. The username is the expiry and an HMAC of the
address under the TURN secret, so it does not show the address. coturn counts
`user-quota` per username, so the quota limits the allocations of each address,
however many credentials it fetches. nginx passes the visitor's address to the
issuer in `X-Forwarded-For`.

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

Coturn permits at most 400 allocations total and 16 per username, which is 16
per address. Every page gathers TURN candidates, whether or not it uses them.
Each of its relay connections takes one allocation per TURN URL (UDP and TCP) on
every relay's coturn. A measured local run held two allocations on each coturn
per open connection. A page on two relays therefore holds four allocations on
each coturn; the other relay's coturn refuses to forward to this relay, so those
stay unused. So 16 serves four pages behind one address, such as an office NAT.
Keep `total-quota` at least 2 x relays x `MAX_PEERS` (400 for two relays at 100
peers), and `user-quota` at least 2 x relays. Past either cap, new pages get no
TURN; direct connections still work.

The quota limits one address, not one attacker. An address holds at most 16
allocations per window, and 32 across a window change, so filling all 400 takes
13 to 25 addresses that keep their allocations refreshed. Rented hosts or an
IPv6 /48 have that many. Such an attacker takes TURN from new visitors on
networks that block UDP. It does not reach anything new: each allocation still
forwards only to this relay's WebRTC ports. Watch the allocation count under
[Budget and monitoring](#budget-and-monitoring). TCP transport to TURN is
allowed; TCP allocations are not. Allocation lifetime is capped at 600 seconds.
No packet-dropping bandwidth cap is configured: previous measurements found that
it caused channel resets. TURN over TLS/443 is not supplied: it needs another IP
or a TLS router.

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
# Only this kit's server blocks: no default site on port 80.
rm -f /etc/nginx/sites-enabled/default
sed "s/<RELAY_DOMAIN>/$RELAY_DOMAIN/g" tools/deploy/minaweb/acme.nginx.conf \
  >/etc/nginx/conf.d/minaweb.conf
nginx -t
systemctl enable --now nginx
systemctl reload nginx
certbot certonly --webroot -w /var/www/acme -d "$RELAY_DOMAIN" \
  --register-unsafely-without-email --agree-tos
sed "s/<RELAY_DOMAIN>/$RELAY_DOMAIN/g" tools/deploy/minaweb/relay.nginx.conf \
  >/etc/nginx/conf.d/minaweb.conf
nginx -t
systemctl reload nginx
# Reload nginx after each successful renewal; certbot.timer runs renewals.
printf '#!/bin/sh\nnginx -t && systemctl reload nginx\n' \
  >/etc/letsencrypt/renewal-hooks/deploy/reload-nginx
chmod 755 /etc/letsencrypt/renewal-hooks/deploy/reload-nginx
SSH_FROM=OWNER_CIDR tools/deploy/relay/firewall.sh
ufw status verbose
install -m 644 tools/deploy/minaweb/mina-turn-egress.service \
  /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now mina-turn-egress
nft list table inet mina_turn
# Only now may the hardened service start. The kit's unit replaces the mask.
systemctl unmask coturn.service
install -m 644 tools/deploy/minaweb/coturn.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now coturn
systemctl is-active --quiet coturn
systemctl show coturn -p User -p Group -p Requires -p BindsTo -p After
python3 -B tools/deploy/minaweb/check-turn-egress.py --ip "$VPS_IPV4" \
  --user-quota 16
python3 -B tools/deploy/minaweb/check-turn-egress.py --ip "$VPS_IPV4" \
  --target listener
nft list table inet mina_turn
# Expected: allowed delivery plus a drop-counter increase of at least two.
# The test used and closed UDP 40000 and 39999; now start the native relay.
systemctl start mina-relay
systemctl is-active --quiet mina-relay
runuser -u mina -- python3 -c \
  'import json, sys; assert json.load(open(sys.argv[1])) == []' \
  /etc/mina-relay/ice-servers.json
```

The local TURN probe works on its own allocations only. It asks the issuer for
the username of a random benchmarking address (198.18.0.0/15), so earlier runs
and visitors do not share its quota. It requires authenticated allocation and
checks that coturn:

- refuses an anonymous allocation and a TCP allocation;
- caps a requested 3600 s lifetime at 600 s;
- ignores an unsigned `LIFETIME` 0 after MESSAGE-INTEGRITY in a signed Refresh
  (CVE-2026-68554; Ubuntu's 4.6.1 fails here);
- refuses permissions to other addresses (192.0.2.1 and 127.0.0.1);
- delivers to a receiver on a WebRTC port, and does not deliver to an unrelated
  UDP receiver on the same allowed IP or to TURN's own STUN listener;
- with `--user-quota 16`, gives one username 16 allocations and refuses the 17th
  (486).

A negative-only check is insufficient. It deallocates before exiting. For repeat
tests with the node running, choose unused WebRTC ports with `--allowed-port`;
never stop or reuse another service's socket. Also run the external forced-TURN
sync gate.

`coturn.service` runs coturn as `turnserver` in a systemd sandbox: no
capabilities, no new privileges, a read-only system, no home directories, a
system-call filter, 256 MiB of memory and 64 tasks. The guard's `ExecStartPre=+`
runs as root outside that sandbox. `systemd-analyze security` rates the three
kit units; record their scores with each install.

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
to a separate TURN server. Each relay host gets the same rules. A Hetzner Cloud
Firewall with the same rules, applied with `hcloud`, adds a second layer.
Ubuntu's `certbot.timer` renews the certificate; the `reload-nginx` hook above
reloads nginx after each successful renewal. Test renewal with
`certbot renew --dry-run --no-random-sleep-on-renew`; without a terminal,
certbot otherwise sleeps up to 8 minutes first.

Let's Encrypt no longer sends expiry emails, so an account email serves only
account recovery. relay1 registered without one
(`--register-unsafely-without-email`). Use `--email OWNER_EMAIL` instead to keep
that recovery path. Either way, certificate expiry is watched by the monitoring
below.

nginx sends `Strict-Transport-Security: max-age=31536000` on every HTTPS
response, errors included. It covers this host name only: no `includeSubDomains`
and no preload, so it says nothing about `minaweb.org` or other relays. It also
sends `nosniff`, `Referrer-Policy: no-referrer` and a `default-src 'none'`
policy. TLS 1.2 uses forward-secret AEAD suites only.

### Logs

The journal is the host's only system log. It keeps 7 days, in at most 1 GiB,
and copies nothing to rsyslog's text files. Turn rsyslog off and delete the text
logs it wrote before:

```bash
install -d /etc/systemd/journald.conf.d
install -m 644 tools/deploy/minaweb/journald-relay.conf \
  /etc/systemd/journald.conf.d/zz-mina-relay.conf
systemctl restart systemd-journald
systemctl mask --now rsyslog.service
rm -f /var/log/syslog* /var/log/auth.log* /var/log/kern.log* \
  /var/log/ufw.log* /var/log/user.log* /var/log/cron.log* /var/log/mail.log*
systemd-analyze cat-config systemd/journald.conf |
  grep -E '^(Storage|MaxRetentionSec|SystemMaxUse|ForwardToSyslog)=' | tail -4
```

The last four lines are this kit's values. The file name sorts after every other
journald drop-in, including rsyslog's `syslog.conf`, which sets
`ForwardToSyslog=yes`; a later drop-in would win.

The journal holds client addresses:

- coturn logs each TURN session with the client's address and port;
- the node logs its peers' addresses, and the visitor's WebRTC offer (with their
  addresses) at debug level only, which the relay does not log;
- sshd logs the owner's address;
- ufw logs blocked packets with their source address.

nginx keeps no access log and no request error log. Its own start-up errors go
to `/var/log/nginx/error.log`, which Ubuntu rotates daily and keeps 14 days;
they hold no client address. Nothing else on the host stores a visitor's
address.

### SSH

Install the SSH settings: root logs in with a key only, and nothing else logs
in. Keep the current session open and test a second login before closing it:

```bash
install -m 644 tools/deploy/minaweb/sshd-relay.conf \
  /etc/ssh/sshd_config.d/10-mina-relay.conf
# Ubuntu starts sshd from a socket; the check needs its run directory.
install -d -m 755 /run/sshd
sshd -t
systemctl reload-or-restart ssh
sshd -T |
  grep -E '^(permitrootlogin|passwordauthentication|authenticationmethods) '
```

Expected: `permitrootlogin without-password` (sshd's name for
`prohibit-password`), `passwordauthentication no` and
`authenticationmethods publickey`. Local port forwarding stays on, for the
operator API tunnel below. Agent, X11 and remote forwarding are off.

Then check the relay from outside, before any page lists it, with an SSH tunnel
to its operator API:

```bash
ssh -N -L 13001:127.0.0.1:3000 OWNER@"$RELAY_DOMAIN" &
python3 -B tools/deploy/minaweb/check.py "/srv/evidence/$RELAY_DOMAIN" \
  --relay-only --relay "https://$RELAY_DOMAIN" --turn-ip "$VPS_IPV4" \
  --relay-http http://127.0.0.1:13001
```

It checks HTTPS signalling, closed operator routes, the credential service (CORS
for `https://minaweb.org`, no-store, expiry, TURN on this relay's address only),
STUN and synced native peers, and writes `RELAY-PASS`. It never fetches the
site.

nginx permits six signalling requests and six credential requests per minute per
source IP, with a burst of ten and eight concurrent requests. Excess requests
receive 429. Shared NAT users share those budgets. Tune from observed launch
traffic; do not silently remove the limits. These are application limits, not
DDoS protection. No visitor offer URLs are logged; nginx request error logging
is disabled too. Monitor service health separately.

### Testing a page on another origin

The credential service sends CORS only for `https://minaweb.org`, so a page
served anywhere else (a local test, a preview hostname) can't read it. Do not
widen its CORS. For such a test, fetch a list on each relay host over SSH
(`curl -s http://127.0.0.1:3002/ice-servers.json`), join them into one file
outside git (`jq -s add relay1.json relay2.json`), and build the test page with
`tools/regression/webnode/build.sh` and `ICE_SERVERS` set to that file. The page
serves it as `/webnode-devnet/ice-servers.json`. Its credentials work for 24
hours: keep the file private and delete it after the test.

## R2 and Pages headers

Set the bucket CORS policy with:

```bash
npx wrangler r2 bucket cors set minaweb-assets \
  --file tools/deploy/minaweb/r2-cors.json
```

This is Wrangler's `rules`/`allowed` format, permitting only
`https://minaweb.org`. The dashboard uses a different format; do not paste this
file there. See the [R2 command reference][r2-commands].

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
`tools/deploy/minaweb/cloudflare-asset-headers.json` holds these rules in the
rulesets API's format; see
[Set up Cloudflare through the API](#set-up-cloudflare-through-the-api).

The generated Pages `_headers` sets COOP `same-origin`, COEP `require-corp`,
CORP `same-origin`, nosniff and cache revalidation. Pages serves normal HTML,
CSS, JavaScript and JSON MIME types. The WASM and circuits stay off Pages; the
recipe rejects any Pages file over 25 MiB. The browser checks the WASM's SHA-256
before compilation, and the WASM checks compiled circuit pins.

The page holds private keys, so `_headers` also sends a strict
Content-Security-Policy, `X-Frame-Options: DENY` and a Permissions-Policy that
turns off camera, microphone, location, payments, USB, serial, HID and MIDI
devices, motion sensors and screen capture. The build makes the policy from its
own origins and records it in `build-info.json`; `release.py` writes it into
`_headers`. `tools/regression/webnode/csp.py` gives the reason for every source.
For minaweb.org with two relays it is, on one line:

```text
default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'
blob:; connect-src 'self' https://assets.minaweb.org https://relay1.minaweb.org
https://relay2.minaweb.org; img-src 'self' blob:; style-src 'self'; font-src
'self'; base-uri 'none'; form-action 'none'; object-src 'none'; frame-ancestors
'none'
```

No inline script, eval or framing is allowed. The page may fetch only from
itself, the asset host and the relays (signalling and the credential services).
CSP does not restrict WebRTC in Chrome (it has no `webrtc` directive), so the
policy does not limit TURN or peer connections; the seed relay allowlist does. A
relay on a new origin needs a new build, not only a new `peers.txt`.

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

`RELAYS` in the release settings lists every relay as `NAME=PEER_ID`. One relay
may instead set `RELAY_PEER_ID` and `RELAY_DOMAIN`, as before.

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

From another network, open an SSH tunnel to each relay's private API. Keep the
tunnels private. Install the browser harness prerequisites, then:

```bash
ssh -N -L 13001:127.0.0.1:3000 OWNER@relay1.minaweb.org &
ssh -N -L 13002:127.0.0.1:3000 OWNER@relay2.minaweb.org &
CHROME=/opt/chrome/chrome PUPPETEER_DIR=/opt/browser-harness \
  python3 -B tools/deploy/minaweb/check.py /srv/evidence/launch \
  --relay https://relay1.minaweb.org --turn-ip RELAY1_IPV4 \
  --relay-http http://127.0.0.1:13001 \
  --relay https://relay2.minaweb.org --turn-ip RELAY2_IPV4 \
  --relay-http http://127.0.0.1:13002
```

Give each relay its `--relay`, `--turn-ip` and `--relay-http`, in that order.
One relay may give its API as `RELAY_HTTP` instead, as before. The build must
dial exactly the relays given.

The gate checks COOP/COEP and WASM plus both verifier caches' CORS/CORP/MIME,
with and without an Origin header. It rebuilds the Content-Security-Policy from
`build-info.json` and the relays and requires the page and a route to serve
exactly it, once, with no unsafe source, plus `X-Frame-Options: DENY` and the
Permissions-Policy. It downloads both caches and checks their source-digest
prefixes against the checkout used to run the gate; use the release's source
commit.

For each relay it checks HTTPS signalling and closed operator routes. It checks
the credential service: no-store, CORS for the site, a username expiry 1 minute
to 24 hours ahead (60 seconds clock tolerance), and TURN on that relay's address
only. It checks a real STUN binding response and synced native devnet peers. The
page's ICE lists together must carry every relay's TURN.

It then runs the locked `run.mjs` harness twice with fresh browsers: normal ICE
and forced TURN. Each run needs a sample Synced within one block of the relays'
tip, with a peer for each relay and an open connection to each relay's address.
In the TURN run each connection uses a relay candidate, so each relay's coturn
carried its own relay's connection. A PASS file is written only after all gates
pass. Add `--automated-only` to stop after the HTTP, credentials, STUN and
native-peer checks: this writes `AUTOMATED-PASS`, never `PASS`, and leaves both
browser sync runs outstanding. The timestamp check is not an HMAC verification;
the forced-TURN run demonstrates the server accepts the issued credentials. A
browser reaching Synced alone is not proof of cache delivery because it can
rebuild missing verifier indexes. A malformed signalling offer alone is not
connectivity evidence. Keep a second real-browser check from a different ISP and
a UDP-restricted network on the owner launch checklist.

Then check that a page survives a relay going down. With a synced page open,
stop one relay (`systemctl stop mina-relay` on relay2). The page must keep one
peer and follow new blocks through the other relay. Start it again; the page
reconnects to it. Repeat with relay1.

## Local two-origin test

Build with `RELAYS=relay1.minaweb.test=PEER_ID` and
`ASSETS_ORIGIN=https://assets.minaweb.test`, using an owned devnet relay's peer
ID. This fixture has one relay. Use a transient user unit for that relay, HTTP
4271, signalling 4272, libp2p 9400 and WebRTC UDP 9401–9409. Do not use an
occupied port or a shared node. Record the native binary's build-info and
SHA-256 before the test.

`tools/deploy/minaweb/local.Caddyfile` serves the page on 4274, assets on 4275
and HTTPS signalling on 4276, all loopback only. Set `RELEASE_DIR` to the
release directory, `RELEASE_PREFIX` to its manifest's `release` field and
`MINAWEB_CSP` to `contentSecurityPolicy` in its `browser/build-info.json`.
Without `MINAWEB_CSP` the page refuses to load. Run Caddy as an unprivileged
owned process with private XDG data/config directories. It creates a private CA
without installing system trust. Obtain its root/intermediate SPKI hashes with
OpenSSL. Set `CA_DIR` to the private data directory's
`caddy/pki/authorities/local`, then run:

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
RULES="MAP minaweb.test 127.0.0.1:4274,MAP assets.minaweb.test 127.0.0.1:4275"
RULES="$RULES,MAP relay1.minaweb.test 127.0.0.1:4276"
PINS="--ignore-certificate-errors-spki-list=$SPKI_HASHES"
BASE=https://minaweb.test PAGE=/ AUTOSTART=1 EVERY_MS=5000 UNTIL_SYNCED=30 \
  CHROME_ARGS="--host-resolver-rules=$RULES $PINS" \
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

For each release, record the full source commit and each relay's binary hash.
Run the same clean build recipe into a new directory, local checks, immutable
upload, header check and Pages deploy. Record the Pages deployment ID and
manifest SHA-256. Keep at least the last two working bundles and their R2
namespaces. Do not mutate the peer list in place: release a new Pages bundle so
its manifest remains accurate. Credentials remain fresh on every boot because
they come from the issuer, including for old deployments.

Rollback Pages by selecting the previous production deployment in the dashboard,
or deploying its retained `browser/` directory with the generated Pages command.
Keep its R2 files. Do not rebuild an old commit and assume identical bytes or
settings. Re-run the external check after rollback.

Update one relay at a time; the page keeps syncing through the other. For relay
updates, install and verify a new directory under `/opt/mina-relay/releases/`,
retaining the old directory, identity and state. Check its full commit. Check
that its `mina node --help` lists `--webrtc-sharing-relay`, or
`--webrtc-signaling-relay` for the 0.20.1 release. Then swap `current`
atomically:

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

## Updates

Each relay installs Ubuntu's security updates by itself.
`/etc/apt/apt.conf.d/52mina-relay` (from `cloud-init.yaml`, or
`tools/deploy/minaweb/unattended-upgrades-relay.conf` on an older host) turns on
the daily `unattended-upgrades` run. When an update needs a reboot, the relay
reboots at its `Automatic-Reboot-Time`: 04:00 UTC on relay1, 05:00 on relay2.
The page keeps syncing through the other relay meanwhile. Services that use an
updated library restart after the update.

coturn is the exception: it comes from [Build coturn](#build-coturn), not apt.
The relay binary comes from a release, as below.

After each automatic reboot, and once a month, check each relay:

```bash
uptime
! test -e /run/reboot-required
apt list --upgradable 2>/dev/null | grep -c upgradable
systemctl is-active mina-relay nginx coturn mina-turn-credentials \
  mina-turn-egress ufw
journalctl -b -o short-monotonic -u ufw -u mina-turn-egress -u coturn |
  grep -E 'Finished|Started'
python3 -B check-turn-egress.py --ip "$VPS_IPV4" --allowed-port 40998
```

The order is ufw, then `mina-turn-egress`, then coturn. The upgradable count is
0, or only packages from `-updates` that are not security fixes. Then run
`check.py --relay-only` from outside.

## Update an installed relay to this kit

A relay installed from an earlier version of this runbook runs Ubuntu's coturn
4.6.1, a drop-in for the egress guard, rsyslog and the stock SSH settings.
relay1 is one. Update it in place; its identity, TURN secret, certificate, state
and firewall stay as they are. Do one relay at a time. This is a one-time
migration: on a relay already updated, step 5 would overwrite the saved 4.6.1
configuration, so don't run it twice.

1. Back up the stopped state, as root, readable by root only:

   ```bash
   systemctl stop mina-relay
   (umask 077 && tar -C /var/lib \
     -cf "/root/state-backup-$(date -u +%Y%m%dT%H%M).tar" mina-relay)
   systemctl start mina-relay
   ```

2. Install this kit's nginx include and reload nginx, as under
   [TLS and firewall](#tls-and-firewall). Do this before the new credential
   service starts: without the include's `X-Forwarded-For`, every visitor gets
   the same username and shares one quota.
3. Install a new release directory with the binary and this kit's
   `start-relay.sh`, `turn-credentials.py` and `turn-egress.py`, as under
   [Routine release and rollback](#routine-release-and-rollback). Install the
   hardened `mina-relay.service` and `mina-turn-credentials.service`. Switch
   `current` and restart both.
4. Install coturn 4.18.0's run-time libraries by name. A host installed before
   this kit lacks `libmicrohttpd12t64`. Naming them also marks them as installed
   by hand: after `apt-get remove coturn`, apt would otherwise count the
   libevent libraries as unused, and unattended upgrades remove unused packages.

   ```bash
   apt-get install libevent-core-2.1-7t64 libevent-extra-2.1-7t64 \
     libevent-openssl-2.1-7t64 libevent-pthreads-2.1-7t64 \
     libmicrohttpd12t64 libssl3t64
   ```

   Then install coturn 4.18.0 under `/opt/coturn` as under
   [Build coturn](#build-coturn). The `turnserver` user already exists; skip
   `useradd`. Check that every library it needs resolves:

   ```bash
   libs=$(ldd /opt/coturn/current/bin/turnserver) ||
     { echo 'ldd failed: is coturn installed?' >&2; false; }
   ! grep 'not found' <<<"$libs"
   ```

5. Write the new `turnserver.conf` with the host's current secret, without
   printing it, then check it with the new guard:

   ```bash
   python3 - "$VPS_IPV4" "$RELAY_DOMAIN" <<'PY'
   from pathlib import Path
   import os, sys
   os.umask(0o077)  # the new file holds the secret: root-only from the start
   secret = Path('/etc/mina-relay/turn.secret').read_text().strip()
   s = Path('tools/deploy/minaweb/turnserver.conf').read_text()
   s = s.replace('<VPS_PUBLIC_IPV4>', sys.argv[1])
   s = s.replace('<RELAY_DOMAIN>', sys.argv[2])
   assert s.count('<TURN_AUTH_SECRET>') == 1
   new = Path('/etc/turnserver.conf.new')
   new.unlink(missing_ok=True)  # never reuse a file left by an interrupted run
   with new.open('x') as f:
       f.write(s.replace('<TURN_AUTH_SECRET>', secret))
   PY
   chown root:turnserver /etc/turnserver.conf.new
   chmod 640 /etc/turnserver.conf.new
   mv /etc/turnserver.conf /root/turnserver.conf.4.6.1
   mv /etc/turnserver.conf.new /etc/turnserver.conf
   python3 /opt/mina-relay/current/turn-egress.py --check
   ```

6. Replace Ubuntu's coturn with the kit's unit. `apt-get remove` keeps the
   `turnserver` user. Never `purge` it: purging deletes that user, whose UID the
   egress guard matches.

   ```bash
   systemctl stop coturn
   apt-get remove -y coturn
   rm -r /etc/systemd/system/coturn.service.d
   install -m 644 tools/deploy/minaweb/coturn.service /etc/systemd/system/
   systemctl daemon-reload
   systemctl enable --now coturn
   ```

   Then run the two TURN probes under [TLS and firewall](#tls-and-firewall),
   with `--allowed-port` set to unused WebRTC ports while the node runs.

7. Follow [Logs](#logs), [SSH](#ssh) and [Updates](#updates) (install
   `unattended-upgrades-relay.conf` as `/etc/apt/apt.conf.d/52mina-relay`).
8. Reboot once, and run the checks under [Updates](#updates) and
   `check.py --relay-only`.

To roll back coturn, point `/opt/coturn/current` at the previous directory.
Ubuntu's 4.6.1 is not a rollback target.

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
features remain separate. Hetzner Cloud includes 20 TB of outbound traffic per
server in its EU locations; recheck the plan when ordering.

Budget each relay's egress as cold visits times bootstrap bytes, plus following
minutes times a measured active rate, plus retries, native gossip and transport
overhead. In one local cold devnet bootstrap with two relays, the page received
136 MiB over both connections together, in line with one relay's figure. Budget
each relay for every visitor's whole bootstrap anyway: one relay serves it all
while the other is down. Each relay sends a connected page its new blocks, so
following traffic is per relay. TURN on the same host adds CPU and overhead but
no second external hop to the relay. At 136 MiB a cold devnet visit, 20 TB is
about 140,000 cold visits per relay before following traffic. Add billing alerts
at 50/75/90 percent and a documented owner decision to reduce admission or
disable new sessions before overage.

Monitor each relay: HTTPS healthz and readyz every minute from outside,
certificate expiry, native connected peers and tip progress via the private API,
systemd restart counts, memory/disk, TURN use and VPS outbound bytes.

coturn's admin CLI is off (the 4.18 default), and its journal lines are per
session, so TURN use comes from two places. The egress guard's counters
(`nft list table inet mina_turn`) show packets and bytes relayed to the relay's
WebRTC range, and the drops. The allocation sockets show open allocations:
`ss -Huan '( sport >= :49152 and sport <= :49999 )' | wc -l`. A quiet chain can
have an old tip; compare a separate devnet source before declaring sync failure.
Run a scheduled real-browser smoke at a budgeted cadence: it consumes bootstrap
bandwidth. Avoid storing offer URLs, ICE credentials or visitor IPs in public
monitoring. Keep operational logs short and private.

## Launch checklist

- [ ] Agent: signed preparation commits, manifest, local split-origin sync
      evidence and export inventory pass; independent review by coordinator.
- [ ] Owner: Cloudflare account/MFA, zone delegation, R2 billing, Direct Upload
      project, bucket-scoped credentials and private rclone configuration ready.
- [ ] Owner: one server per relay (fsn1, hel1) with a public IPv4, each relay's
      DNS-only record resolves, budget alerts configured, SSH/console access
      tested.
- [ ] Owner, each relay: qualified post-173 relay binary/hash, stable peer ID,
      private identity backup, native peers and readyz verified.
- [ ] Owner, each relay: nginx certificate/renewal, firewall, its own TURN
      secret, issuer and quotas installed; operator routes return 404
      externally.
- [ ] Owner, each relay: coturn 4.18.0 from `build-coturn.sh`, both TURN probes
      pass, logs, SSH and automatic updates as in this runbook, and a reboot
      checked.
- [ ] Agent: build from the approved commit with the owner's public values;
      review manifest and exact deployment commands.
- [ ] Owner: R2 immutable upload and download verification; CORS/CORP/MIME
      correct with and without Origin; assets readable before Pages deploy.
- [ ] Owner: Pages deployment ID recorded; canonical domain attached;
      crossOriginIsolated true in a fresh browser.
- [ ] Agent: external `check.py` PASS for every relay, direct and forced-TURN
      synced tips; owner checks another ISP and a restricted network, and a page
      surviving each relay's stop.
- [ ] Owner: monitoring, cost alerts, rollback bundle and prior R2 namespace
      retained; short-TTL renewal and issuer-outage behavior checked.
- [ ] Owner: decide whether each relay runs `SHARING_RELAY=on`
      ([Browser sharing](#browser-sharing)); off needs nothing.
- [ ] Owner: approve launch announcement only after all gates pass.

[r2-rclone]: https://developers.cloudflare.com/r2/examples/rclone/
[coturn-advisories]: https://github.com/coturn/coturn/security/advisories
[r2-commands]: https://developers.cloudflare.com/r2/reference/wrangler-commands/
