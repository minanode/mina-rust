# The public site: minaweb.org

The web node's pages are static files. Three things make serving them different
from an ordinary site:

- **Cross-origin isolation.** The node runs on threads (WASM with shared
  memory), which a browser allows only on a page sent with
  `Cross-Origin-Opener-Policy: same-origin` and
  `Cross-Origin-Embedder-Policy: require-corp`. Every file the page loads from
  another origin must then come with CORS (or
  `Cross-Origin-Resource-Policy: cross-origin`).
- **Big files.** The WASM is about 35 MB (3.4 MB with brotli). Circuit files
  come from the same place as the verifier index caches (2 files, 3.3 MB each);
  a block producer or SNARK worker also loads the prover circuits (devnet: 24
  files, 130 MB in all, the largest 23 MB).
- **HTTPS everywhere.** A page served over HTTPS can only call HTTPS URLs: the
  relay's signalling is `/dns4/relay.minaweb.org/tcp/443/webrtc/https/...`
  ([`../relay`](../relay/README.md)).

**GitHub Pages can't serve the site:** it sends no custom headers, so the page
can't be cross-origin isolated and the node doesn't start. (A service worker
that adds the headers, such as coi-serviceworker, works around it at the cost of
a reload on the first visit; not used here.)

The plan: **Cloudflare Pages** for the pages, **Cloudflare R2** (a bucket behind
a custom domain) for the WASM and the circuit files, the relay on its VPS. Both
Cloudflare services are free at this size (Pages: unlimited requests and
bandwidth; R2: no egress fees, 10 GB stored free) and keep the site up when the
VPS is down. Netlify, or Caddy on the VPS, are alternatives below.

| File                       | For                                                                          |
| -------------------------- | ---------------------------------------------------------------------------- |
| `build.sh`                 | builds the public devnet bundle: relay over HTTPS, asset host, ICE servers   |
| `_headers`                 | Cloudflare Pages and Netlify: COOP, COEP, CORP on every file                 |
| `_redirects`               | Netlify only: the frontend's routes to `index.html`                          |
| `r2-cors.json`             | the R2 bucket's CORS policy                                                  |
| `ice-servers.example.json` | the shape of the page's STUN and TURN list (the real one holds the password) |
| `site.caddy`               | the site from Caddy instead, with the same headers                           |

## Build

On the build machine, with a merged commit checked out, its own Cargo target
directory and the relay's peer id
([`../relay`](../relay/README.md#the-peer-id)):

```bash
cp tools/deploy/site/ice-servers.example.json /path/outside/git/ice-servers.json
$EDITOR /path/outside/git/ice-servers.json      # the TURN password, or drop TURN
tools/regression/lock.sh build -- env CARGO_BUILD_JOBS=8 nice -n 10 \
    CARGO_TARGET_DIR=$PWD/target-web RELAY_PEER_ID=12D3KooW... \
    ICE_SERVERS=/path/outside/git/ice-servers.json \
    tools/deploy/site/build.sh
```

`make build-webnode-public` runs the same script (with the same environment). It
prints where the bundle is (`OUT`, by default
`.work/webnode/builds/<commit>-devnet-public`): `OUT/browser` is the site and
`OUT/upload` the files for the asset host. `OUT/browser/build-info.json` names
the commit, the relay, the asset host and the WASM's digest. Settings, all with
minaweb.org defaults, are at the top of `build.sh`: `RELAY_DOMAIN`, `ASSETS_URL`
(`https://assets.minaweb.org`), `HOSTING` (`pages`, `netlify` or `caddy`).

The ICE servers name the VPS by its IPv4 address, not a host name: R9 measured
Chrome taking 40 s to give up on a STUN host name's IPv6 address on a host with
IPv6 addresses but no IPv6 route, while the IPv4 literal answered in 133 ms. The
node doesn't trickle ICE and waits at most 3 s for gathering, so one slow server
delays every connection by 3 s. For the same reason the list holds only servers
that answer: the node's built-in list, used without `ICE_SERVERS`, has two dead
ones.

Two files in the site can change without a rebuild: `webnode-devnet/peers.txt`
(the relay's address) and `webnode-devnet/ice-servers.json` (STUN and TURN). The
page reads both at start.

## Cloudflare Pages

Pages takes no file over 25 MiB, which is why `HOSTING=pages` (the default)
moves the WASM to the asset host and the glue loads it from there.

1. Create a Pages project by direct upload (no Git integration: Pages' build
   machines can't build the WASM), named for example `minaweb`.
2. Deploy:
   `npx wrangler pages deploy OUT/browser --project-name minaweb --branch main`.
   The bundle is about 360 files.
3. Custom domain: `minaweb.org` (and `www.minaweb.org`, redirected to it with a
   Bulk Redirect, so that the page has one origin).
4. Pages serves `index.html` for unknown paths by itself (there is no
   `404.html`), which the frontend's routes need.

## The asset host

An R2 bucket, `minaweb-assets`, with the custom domain `assets.minaweb.org` (R2,
the bucket, Settings, Custom Domains), holding `OUT/upload` at the same paths:
`wasm/<name>.wasm` and `circuit-blobs/4.0.2devnet/...`.

**The asset host is not trusted.** The page, served from Pages, pins the WASM's
URL and SHA-256 (`build-info.json` shows them), fetches it, checks it with
WebCrypto and refuses to start the node on other bytes; the prover's iframe does
the same. The SHA-256 of every circuit file the build ships is compiled into the
WASM, which refuses a file without its pin or with other bytes. So after a new
build, upload its files before (or with) its site: a site whose WASM isn't on
the asset host yet doesn't start, it never runs an older one. For a producer's
or SNARK worker's page, build with `PROVER_CIRCUITS` (the prover circuit
directory) so that those files are shipped and pinned too; a build without them
refuses to load any.

1. **CORS**: the bucket's policy is `r2-cors.json` (Settings, CORS policy).
   Cloudflare's cache doesn't vary on `Origin`, so also add a Response Header
   Transform Rule on the zone, for hostname `assets.minaweb.org`, that sets
   `Access-Control-Allow-Origin: https://minaweb.org` and
   `Cross-Origin-Resource-Policy: cross-origin` on every response: then a cached
   copy fetched without `Origin` still carries them.
2. **The WASM**, compressed (its name is its digest, so it can be kept forever):

   ```bash
   N=$(cd OUT/upload/wasm && ls mina_node_web_bg.*.wasm)
   npx wrangler r2 object put minaweb-assets/wasm/$N --remote \
       --file OUT/upload/wasm/$N.br --content-type application/wasm \
       --content-encoding br --cache-control 'public, max-age=31536000, immutable'
   ```

3. **Circuit files**: the verifier index caches the build put in
   `OUT/upload/circuit-blobs/4.0.2devnet/`, and for a producer or SNARK worker
   the separately supplied devnet prover circuits:

   ```bash
   for f in OUT/upload/circuit-blobs/4.0.2devnet/*; do
       npx wrangler r2 object put minaweb-assets/circuit-blobs/4.0.2devnet/$(basename $f) \
           --remote --file "$f" --cache-control 'public, max-age=86400'
   done
   ```

   A new build that embeds another verifier index ships other caches under the
   same names: the node checks each against the index it embeds and rebuilds the
   index from source when they differ (about 9 s), so an old cache slows a start
   down but never breaks it. Upload a build's caches with its site.

## Check

```bash
curl -sI https://minaweb.org/ | grep -i cross-origin           # COOP, COEP, CORP
curl -sI -H 'Origin: https://minaweb.org' -H 'Accept-Encoding: br' \
    https://assets.minaweb.org/wasm/$N | grep -iE 'access-control|content-(type|encoding|length)'
curl -s https://minaweb.org/build-info.json
```

Then open https://minaweb.org/loading-web-node, start the node, and in the
browser's console `crossOriginIsolated` is `true`; the network tab shows the
WASM and the verifier caches from `assets.minaweb.org` with status 200, and the
signalling requests to `relay.minaweb.org`.

## Netlify instead

`HOSTING=netlify` keeps the WASM in the bundle (Netlify documents no per-file
limit as low as Pages'; if a deploy refuses it, use `HOSTING=pages` and the
asset host) and copies `_headers` and `_redirects`, which Netlify reads from the
deployed directory: `netlify deploy --dir OUT/browser --prod`. The asset host
for the circuit files is the same R2 bucket (or set `CIRCUITS_URL=` to keep them
in the bundle).

## The VPS instead

`HOSTING=caddy` keeps everything in the bundle, circuit files included. Copy
`OUT/browser` to `/srv/minaweb` on the relay's VPS, install `site.caddy` next to
the `Caddyfile` and uncomment its `import`. Caddy then sends the headers, serves
the compressed WASM the build made, and gets the certificate. One machine to
run, but the site goes down with the relay, and the WASM's 3.4 MB per first
visit counts on the VPS's traffic. Cloudflare's proxy (orange cloud) in front of
`minaweb.org` would cache it and keeps the headers Caddy sends.
