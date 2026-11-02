# The public relays

The web node at https://minaweb.org joins devnet through its relays: native
mina-rust nodes it reaches with WebRTC. It dials every relay the page lists, so
it keeps syncing while one is down. At launch there are two, on Hetzner Cloud:
`relay1.minaweb.org` (fsn1) and `relay2.minaweb.org` (hel1). Each relay's host
stands alone, with its own key, TURN secret, certificate and firewall.

**Install, update and check a relay with
[`docs/release/MINAWEB.md`](../../../docs/release/MINAWEB.md).** It is the one
supported setup: nginx for TLS, signalling and the credential route, coturn with
expiring TURN credentials, and the TURN egress guard. The site itself is in
[`../site`](../site/README.md).

On each relay host:

| What                       | Where                                   | Ports                                                                                              |
| -------------------------- | --------------------------------------- | -------------------------------------------------------------------------------------------------- |
| Relay node (`mina`)        | `mina-relay.service`, `/opt/mina-relay` | 3000/tcp HTTP and 3001/tcp signalling (both on 127.0.0.1), 8302/tcp libp2p, 40000-40999/udp WebRTC |
| nginx                      | `/etc/nginx/conf.d/minaweb.conf`        | 80/tcp (ACME, redirect), 443/tcp: TLS, signalling, `/ice-servers.json`, rate limits                |
| TURN credentials           | `mina-turn-credentials.service`         | 3002/tcp on 127.0.0.1, behind nginx                                                                |
| coturn 4.18.0 (TURN, STUN) | `coturn.service`, `/opt/coturn/current` | 3478/udp and 3478/tcp; allocation ports 49152-49999 (closed)                                       |
| TURN egress guard          | `mina-turn-egress.service` (nftables)   | lets coturn send only to this relay's WebRTC ports                                                 |
| Settings, key, TURN secret | `/etc/mina-relay/`                      |                                                                                                    |
| Node data, kept on update  | `/var/lib/mina-relay/`                  |                                                                                                    |

The node's HTTP port binds `127.0.0.1` (`--http-addr`): its whole API is for the
operator, over SSH. Browsers' signalling goes to a second listener on
`127.0.0.1`, `--webrtc-signal-port` (`WEBRTC_SIGNAL_PORT` in `relay.env`), which
serves only the two signalling routes. nginx proxies signalling there, the
health checks to the HTTP port, and `/ice-servers.json` to the credential
service.

A browser fetches `https://relay1.minaweb.org/mina/webrtc/signal/...`, gets the
node's answer, and connects to the node's WebRTC UDP ports directly. On a
network that drops UDP it goes through that relay's coturn, with a credential
from that relay's `/ice-servers.json`.

The files here, all plain, no secrets (`<...>` are placeholders):

- `mina-relay.service`: the systemd unit. Restarts on failure; reads the key
  from a file at each start, never in argv; keeps a frontier store.
- `start-relay.sh`: what the unit runs: reads the key, then runs the node with
  the settings of `relay.env`.
- `relay.env`: the node's settings: ports, peers, ICE servers, WebRTC UDP ports,
  the public address.
- `ice-servers.json`: the relay's own ICE servers: none (`[]`), as its address
  is public.
- `firewall.sh`: the ufw rules.
- `build-relay.sh`: builds `mina` from a reviewed commit, when not using the
  0.20.1 release.
- `Caddyfile`, `relay.caddy`, `turnserver.conf`: configuration for a local test
  stack, with a static TURN password. Not for a public relay.

nginx, coturn's public configuration, the credential service and the egress
guard are in [`../minaweb`](../minaweb).

<a id="the-peer-id"></a>**The peer id** of a relay, for the site's build (its
`RELAYS` entry, `NAME=PEER_ID`), without showing the key:

```bash
sudo sh -c 'MINA_P2P_SEC_KEY=$(cat /etc/mina-relay/p2p.key) \
    /opt/mina-relay/current/mina keys p2p-key-pair' | sed -n 's/^libp2p_id: *//p'
```

The relay's address for web nodes is then
`/dns4/relay1.minaweb.org/tcp/443/webrtc/https/p2p/<peer id>`.

## Check

```bash
systemctl status mina-relay nginx coturn mina-turn-credentials mina-turn-egress
journalctl -u mina-relay -f            # "WebRTC settings", then sync
/opt/mina-relay/current/mina node status --node http://127.0.0.1:3000 --check
curl -s -o /dev/null -w '%{http_code}\n' https://relay1.minaweb.org/readyz   # 200 when synced
curl -s -o /dev/null -w '%{http_code}\n' https://relay1.minaweb.org/status   # 404: not exposed
curl -s -m 5 http://relay1.minaweb.org:3000/status                          # times out: closed
sudo ss -ltnup | grep -E 'mina|nginx|turnserver|python3'
```

From outside, `tools/deploy/minaweb/check.py --relay-only` checks a relay before
any page lists it (MINAWEB.md, "TLS and firewall").

The node takes a few minutes to sync from scratch and less after a restart (the
frontier store). Then open https://minaweb.org, start the web node and watch it
sync: its peers list shows every relay.

**Logs.** A signalling request's URI carries the visitor's WebRTC offer, with
their IP addresses: the node logs it at debug only, and nginx keeps no access or
request log. The journal is the only system log, kept 7 days; coturn logs each
TURN session's client address there (MINAWEB.md, "Logs").

## Upgrade, roll back

MINAWEB.md, "Routine release and rollback". Update one relay at a time: web
nodes keep syncing through the others, and reconnect to the restarted one by
themselves.

## Change the key

A new key is a new peer id. Make it as in MINAWEB.md, restart the relay, then
update the site: its `webnode-devnet/peers.txt` and, at the next build, this
relay's `RELAYS` entry. Until then, pages loaded before the change can't
reconnect to this relay; they keep the others.

## TURN

Most visitors don't need TURN: each relay has a public address, so a browser
behind any NAT that lets UDP out connects to it directly. TURN is for networks
that drop outgoing UDP (some offices, some VPNs).

- **Credentials.** Each relay's credential service gives every page a username
  and an HMAC credential for that relay's coturn, valid 12 to 24 hours. Pages on
  one address get the same username, so coturn's `user-quota` is per address.
  The page fetches one from each relay and renews each before it expires. The
  signing secret stays on its host; relays share none.
- **Reach.** Each coturn relays to its own host's relay node only:
  `allowed-peer-ip` is the host's address, and the egress guard limits coturn's
  output to the relay's WebRTC ports. A stranger with a credential reaches only
  what they could reach without it.
- **Ports.** TURN listens on 3478, UDP and TCP. Its allocation ports
  (49152-49999) carry traffic to the relay node on the same host only, so they
  stay closed. TURN over TLS (`turns:`) would need 443, which is nginx's: a
  second IPv4 address or a TLS router. Left for later.
- **Cost.** TURN forwards a visitor's whole connection (still encrypted end to
  end). With TURN on the relay's host, what the relay sends leaves the VPS once,
  as it would without TURN: no extra traffic on the bill, a little CPU.
  `total-quota` caps allocations. No bandwidth caps (`max-bps`, `bps-capacity`):
  coturn enforces them by dropping packets, which closed the web node's data
  channels again and again in W1's test.
- **Abuse.** UDP allocations only (`no-tcp-relay`): a TCP allocation to the
  VPS's own address would reach ports the firewall closes, the node's HTTP API
  among them.
