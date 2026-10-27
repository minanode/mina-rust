# The public relay: runbook

The web node at https://minaweb.org joins devnet through one native mina-rust
node, the relay, which it reaches with WebRTC. This directory is the kit to run
that relay on a rented VPS. The design, its tests and its costs are in private
implementation notes; the site itself is in [`../site`](../site/README.md).

On the VPS:

| What                      | Where                                    | Ports                                                                                              |
| ------------------------- | ---------------------------------------- | -------------------------------------------------------------------------------------------------- |
| Relay node (`mina`)       | `mina-relay.service`, `/opt/mina-relay`  | 3000/tcp HTTP and 3001/tcp signalling (both on 127.0.0.1), 8302/tcp libp2p, 40000-40999/udp WebRTC |
| Caddy                     | `caddy.service`, `/etc/caddy`            | 80/tcp, 443/tcp and 443/udp: TLS for the relay's signalling                                        |
| coturn (TURN and STUN)    | `coturn.service`, `/etc/turnserver.conf` | 3478/udp and 3478/tcp; relay ports 49152-49999 (closed, local)                                     |
| Settings and the key      | `/etc/mina-relay/`                       |                                                                                                    |
| Node data, kept on update | `/var/lib/mina-relay/`                   |                                                                                                    |

The node's HTTP port binds `127.0.0.1` by default (`--http-addr`): its whole API
is for the operator, over SSH. The browsers' signalling goes to a second
listener, `--webrtc-signal-port` (`WEBRTC_SIGNAL_PORT` in `relay.env`, also on
`127.0.0.1`), which serves only the two signalling routes (RL1): Caddy proxies
the signalling there and only the health checks to the HTTP port. A relay
without a local reverse proxy must set `--webrtc-signal-addr 0.0.0.0` (or a
reachable interface address) for browser nodes to reach signalling, and can then
serve only pages over plain HTTP.

A browser fetches `https://relay.minaweb.org/mina/webrtc/signal/...` (Caddy,
which passes that path to the node's signalling listener and `/healthz`,
`/readyz` to its HTTP port), gets the node's answer, and then connects to the
node's WebRTC UDP ports directly, or through coturn when its network drops UDP.

The files here, all plain, no secrets (`<...>` are placeholders):

- `mina-relay.service`: the systemd unit. Restarts on failure; reads the key
  from a file at each start, never in argv; keeps a frontier store.
- `start-relay.sh`: what the unit runs: reads the key, then runs the node with
  the settings of `relay.env`.
- `relay.env`: the node's settings: ports, peers, ICE servers, WebRTC UDP ports,
  the public address.
- `ice-servers.json`: the relay's own ICE servers: none (`[]`), as its address
  is public.
- `Caddyfile`, `relay.caddy`: Caddy's settings: TLS from Let's Encrypt and the
  reverse proxy to the signalling.
- `turnserver.conf`: coturn's settings.
- `firewall.sh`: the ufw rules.

## Provision

1. **The VPS.** Rented in the project's name, Ubuntu 24.04 LTS (the release
   binary is built on 24.04: glibc 2.39, OpenSSL 3), x86-64 with AVX2. Devnet
   needs about 4 vCPU, 8 GB RAM (the devnet relay on the build machine uses 1.0
   GB after half an hour; give it room to grow) and 40 GB of disk. A public IPv4
   address on the interface (Hetzner, OVH, DigitalOcean) is simplest; on AWS or
   GCP, whose interfaces have private addresses, set `WEBRTC_NAT_IP` and
   coturn's `external-ip` (below). A provider that counts only outgoing traffic,
   with a few TB included, keeps bandwidth out of the bill.
2. **DNS.** An `A` record (and `AAAA` if the VPS has IPv6) for
   `relay.minaweb.org` to the VPS. With DNS on Cloudflare, leave it **DNS only**
   (grey cloud): TURN and the WebRTC UDP ports can't go through Cloudflare's
   proxy, and Caddy gets its own certificate.
3. **Packages and the user:**

   ```bash
   sudo apt update && sudo apt install -y caddy coturn ufw
   sudo useradd --system --user-group --home-dir /var/lib/mina-relay \
       --shell /usr/sbin/nologin mina
   ```

   Ubuntu's Caddy (2.6) is the version the kit was tested with; Caddy's own apt
   repository has newer ones and the same configuration works.

## Install

1. **A release**, built on the build machine from a merged commit, in one lock
   turn:

   ```bash
   tools/regression/lock.sh build -- env CARGO_BUILD_JOBS=8 nice -n 10 \
       tools/deploy/relay/build-relay.sh
   C=$(git rev-parse --short=12 HEAD)
   mkdir -p /tmp/mina-relay-$C && cp target/release/mina tools/deploy/relay/* /tmp/mina-relay-$C/
   scp -r /tmp/mina-relay-$C <vps>:
   ```

   `build-relay.sh` builds `mina` with the relay's WebRTC backend
   (`RELAY_WEBRTC`, see the script) and checks that it names the commit.

   On the VPS:

   ```bash
   C=<commit12>
   sudo install -d /opt/mina-relay/releases
   sudo cp -r ~/mina-relay-$C /opt/mina-relay/releases/$C
   sudo chmod 755 /opt/mina-relay/releases/$C/{mina,start-relay.sh}
   sudo ln -sfn releases/$C /opt/mina-relay/current
   ```

2. **Settings:**

   ```bash
   sudo install -d -m 750 /etc/mina-relay
   sudo install -m 644 /opt/mina-relay/current/relay.env /etc/mina-relay/
   sudo install -m 644 /opt/mina-relay/current/ice-servers.json /etc/mina-relay/
   sudoedit /etc/mina-relay/relay.env    # EXTERNAL_IP; WEBRTC_NAT_IP on AWS/GCP
   ```

3. **The libp2p key**, made on the VPS and never copied elsewhere except to a
   password manager as a backup. It fixes the relay's peer id, which every web
   build names; a new key means a new peer id (see "Change the key").

   ```bash
   sudo sh -c 'umask 077; /opt/mina-relay/current/mina keys p2p-key-pair |
       sed -n "s/^secret key: *//p" > /etc/mina-relay/p2p.key'
   sudo test -s /etc/mina-relay/p2p.key && echo key written
   ```

   <a id="the-peer-id"></a>**The peer id**, for the site's build
   (`RELAY_PEER_ID`), without showing the key:

   ```bash
   sudo sh -c 'MINA_P2P_SEC_KEY=$(cat /etc/mina-relay/p2p.key) \
       /opt/mina-relay/current/mina keys p2p-key-pair' | sed -n 's/^libp2p_id: *//p'
   ```

   The relay's address for web nodes is then
   `/dns4/relay.minaweb.org/tcp/443/webrtc/https/p2p/<peer id>`.

4. **The node:**

   ```bash
   sudo install -m 644 /opt/mina-relay/current/mina-relay.service /etc/systemd/system/
   sudo systemctl daemon-reload
   sudo systemctl enable --now mina-relay
   ```

5. **Caddy.** If the names differ from the minaweb.org ones, set them in a
   drop-in (`sudo systemctl edit caddy`):
   `Environment=RELAY_DOMAIN=relay.example.org ACME_EMAIL=you@example.org`.
   `RELAY_SIGNAL_PORT` (default 3001) must be `relay.env`'s
   `WEBRTC_SIGNAL_PORT`, and `RELAY_HTTP_PORT` (default 3000) its `HTTP_PORT`.

   ```bash
   sudo install -m 644 /opt/mina-relay/current/{Caddyfile,relay.caddy} /etc/caddy/
   sudo systemctl reload caddy
   ```

6. **coturn** (see [TURN](#turn) first: it is optional):

   ```bash
   sudo install -m 640 -g turnserver /opt/mina-relay/current/turnserver.conf /etc/turnserver.conf
   sudoedit /etc/turnserver.conf   # <VPS_PUBLIC_IPV4> three times; <TURN_PASSWORD>
   sudo systemctl restart coturn
   ```

   Make the password with `openssl rand -hex 16`, and put the same one in the
   site's `ice-servers.json` (`../site/ice-servers.example.json`).

7. **The firewall**, with a console open in case SSH is cut:

   ```bash
   sudo SSH_FROM=<your address or range> /opt/mina-relay/current/firewall.sh
   ```

## Check

```bash
systemctl status mina-relay caddy coturn
journalctl -u mina-relay -f            # "WebRTC settings", then sync
/opt/mina-relay/current/mina node status --node http://127.0.0.1:3000 --check
curl -s -o /dev/null -w '%{http_code}\n' https://relay.minaweb.org/readyz   # 200 when synced
curl -s -o /dev/null -w '%{http_code}\n' https://relay.minaweb.org/status   # 404: not exposed
curl -s -m 5 http://relay.minaweb.org:3000/status                          # times out: closed
sudo ss -ltnup | grep -E 'mina|caddy|turnserver'
```

The node takes a few minutes to sync from scratch and less after a restart (the
frontier store). Then open https://minaweb.org, start the web node and watch it
sync: its peers list shows the relay. To check TURN from a browser, open
WebRTC's Trickle ICE sample page with
`turn:<VPS_PUBLIC_IPV4>:3478?transport=udp`, user `webnode` and the password: a
`relay` candidate with the VPS's address means TURN answers.

**Logs.** A signalling request's URI carries the visitor's WebRTC offer, with
their IP addresses: the node logs it at debug only (info shows
`/mina/webrtc/signal/<offer>`), and Caddy keeps no access log. Caddy's error log
still records the URI of a request it couldn't pass on (a 502 while the node is
down), and journald keeps it: limit how long (`MaxRetentionSec=14day` in
`/etc/systemd/journald.conf`, then `sudo systemctl restart systemd-journald`).

## Upgrade

Build and copy a release as in [Install](#install), step 1, then:

```bash
readlink /opt/mina-relay/current          # note the release in use, for a rollback
sudo cp -r ~/mina-relay-$C /opt/mina-relay/releases/$C
sudo chmod 755 /opt/mina-relay/releases/$C/{mina,start-relay.sh}
sudo ln -sfn releases/$C /opt/mina-relay/current
sudo systemctl restart mina-relay
journalctl -u mina-relay -f               # restores from the frontier store, then syncs
```

Web nodes connected at the restart reconnect by themselves. Compare the new
release's `relay.env`, `Caddyfile`, `relay.caddy`, `turnserver.conf` and unit
with the installed ones (`diff`) and carry over what changed.

## Roll back

```bash
ls /opt/mina-relay/releases
sudo ln -sfn releases/<previous> /opt/mina-relay/current
sudo systemctl restart mina-relay
```

A frontier store written by a newer release that the older one can't read is
refused with a reason in the log, and the node bootstraps instead: no data to
repair. Keep the last few releases; delete older ones from
`/opt/mina-relay/releases`.

## Change the key

A new key is a new peer id. Make it as in [Install](#install), step 3, restart
the relay, then update the site: its `webnode-devnet/peers.txt` (served as a
file, no rebuild needed) and, at the next build, `RELAY_PEER_ID`. Until then,
pages loaded before the change can't reconnect.

## TURN

Most visitors don't need TURN: the relay has a public address, so a browser
behind any NAT that lets UDP out connects to it directly. TURN is for networks
that drop outgoing UDP (some offices, some VPNs), and for web nodes connecting
to each other, which this deployment doesn't depend on.

- **Credentials.** The web node gets TURN's user and password from the page, so
  they are public. `turnserver.conf` therefore lets TURN relay to the relay
  node's own address only (`denied-peer-ip` everything, `allowed-peer-ip` the
  VPS): a stranger with the credential can reach only what they could reach
  without it. That makes a long-term credential (`lt-cred-mech`) good enough.
  The alternative, coturn's REST scheme (`use-auth-secret` and
  `static-auth-secret`), gives each visitor a credential that expires, minted by
  a small HTTP service holding the secret (`username = <expiry>:<id>`,
  `password = base64(HMAC-SHA1(secret, username))`). It is worth its service
  only if TURN is opened to other peers.
- **Ports.** TURN listens on 3478, UDP and TCP. Its relay ports (49152-49999)
  carry traffic to the relay node on the same host only, over the loopback
  interface, so they stay closed. TURN over TLS (`turns:`) would help on
  networks that allow only 443, but 443 is Caddy's: it needs a second IPv4
  address for coturn, or a TLS router in front of both (Caddy's layer4 plugin).
  Left for later.
- **Cost.** TURN forwards a visitor's whole connection (still encrypted end to
  end: it can't read it). With TURN on the relay's host, the hop between them
  stays inside the host, so what the relay sends that visitor leaves the VPS
  once, as it would without TURN: no extra traffic on the bill, a little CPU.
  `total-quota` (200 sessions) caps it. No bandwidth caps (`max-bps`,
  `bps-capacity`): coturn enforces them by dropping packets, which closed the
  web node's data channels again and again in W1's test.
- **Abuse.** UDP allocations only (`no-tcp-relay`): a TCP allocation to the
  VPS's own address would reach ports the firewall closes, the node's HTTP API
  among them.
