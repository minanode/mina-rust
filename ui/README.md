# ui

The minanode.org website, the node dashboard, the web node page, and the two
packages they share: the design system and a client for the node's API. An npm
workspace on Node 24.11 (`.nvmrc`), kept apart from the Angular `frontend/` and
the Docusaurus `website/`.

```
ui/
  design-system/   @minanode/design-system: tokens, shadcn/ui components,
                   brand and app patterns, fonts and the showcase page
  node-client/     @minanode/node-client: typed client for the node's HTTP API
                   and GraphQL, with React hooks for polling
  minanode-site/   minanode.org: Next.js 16, static export to out/
  dashboard/       the node dashboard: Vite + React SPA, built to dist/, which
                   `mina node` embeds and serves at `/`
  minaweb/         the web node page: Vite + React SPA that runs the node's
                   WASM in the browser, for minaweb.org
```

The two packages ship TypeScript source and know nothing about each other. The
design system has no idea where data comes from, and the node client draws
nothing. An app pairs them: the dashboard's pages are patterns filled from
client hooks. Both are private workspace packages, consumed by path with no
build or publish step.

## Commands

Run from `ui/`:

| Command                   | What it does                                                |
| ------------------------- | ----------------------------------------------------------- |
| `npm ci`                  | Install the pinned dependencies                             |
| `npm run dev`             | Next.js dev server on port 4271                             |
| `npm run build`           | Static export to `minanode-site/out/`                       |
| `npm run serve`           | Serve `out/` on 127.0.0.1:4270                              |
| `npm run typecheck`       | TypeScript, every workspace                                 |
| `npm run check:contrast`  | WCAG AA colour pairs in both themes, from `tokens.css`      |
| `npm run check:claims`    | Built pages against `minanode-site/CLAIMS.md` (build first) |
| `npm run dev:dashboard`   | Dashboard dev server on port 4282, forwarding to a node     |
| `npm run build:dashboard` | Dashboard build to `dashboard/dist/` (`make dashboard`)     |
| `npm run dev:minaweb`     | Web node page dev server on 4284 (`MINAWEB_ASSETS`)         |
| `npm run build:minaweb`   | Web node page build to `minaweb/dist/` (needs a WASM)       |
| `npm test`                | Unit tests (Vitest), every workspace that has them          |

`out/` is plain files with no server, and nothing on the pages loads from
another origin: fonts are self-hosted and there is no CDN or analytics. Each
route is a directory with an `index.html` (`trailingSlash`), so any static host
serves it without rewrites.

## Design system

`@minanode/design-system` ships source: TypeScript components and Tailwind v4
CSS. Each app compiles it, so there is no build step and nothing to publish.

- `src/styles/tokens.css`: the tokens. Primitives (teal, ink and signal scales),
  shadcn/ui semantic colours for the dark (default) and light themes, brand and
  evidence-status roles, radius, type scale, spacing, effects and motion.
- `src/styles/globals.css`: the entry stylesheet. Tailwind, fonts, tokens,
  forced-state variants for the showcase, base styles and reduced motion.
- `src/components/`: shadcn/ui components (new-york style) restyled through the
  tokens: button, badge, card, tabs, table, alert, separator, input, label,
  skeleton.
- `src/patterns/`: brand pieces (logo, status badge, metric card, stat,
  key-value receipt, eyebrow, section heading, external link, theme toggle) and
  app patterns: the app shell with sidebar navigation, page header and section,
  banner, data age, loading card, role card, signal (live state), hash and copy
  button (copy that also works on plain http), code block, JSON tree, inline
  confirm, segmented control, text link, relative time, amount and empty state.
- `src/showcase/`: the design-system page, mounted by the site at
  `/design-system/`. It reads token values from the live CSS and measures
  contrast in the browser, and shows every pattern in both themes and all its
  states.

[`design-system/README.md`](design-system/README.md) has a usage note for each
component and pattern: its props and an example.

Components take every theme difference from the tokens and use no `dark:`
utilities, so `.dark` or `.light` on any element themes that subtree. After
`npx shadcn@latest add <component>` (run in `design-system/`; `components.json`
sets the aliases), replace the `dark:` utilities it brings with tokens.

### Using it from an app

The node dashboard (`dashboard/`) is a Vite + React SPA that uses it this way,
and any other app under `ui/` can do the same:

1. Add `"@minanode/design-system": "0.1.0"` to its `dependencies` and the
   workspace to `ui/package.json`.
2. Use `@vitejs/plugin-react` and `@tailwindcss/vite`.
3. In its CSS: `@import "@minanode/design-system/styles.css";`
4. Put `<html lang="en" class="dark">` in `index.html` and run `themeInitScript`
   from `@minanode/design-system/lib/theme` in the head. The dashboard emits it
   as `/theme-init.js` rather than inline, so the node's Content-Security-Policy
   can stay `script-src 'self'`.
5. Import components by path, for example
   `@minanode/design-system/components/button` or
   `@minanode/design-system/patterns/status-badge`.
6. Ship the font licences with the build: the OFL notices are in
   `design-system/src/fonts/LICENSE-*.txt`. The site and the dashboard publish
   them, unlinked, at `/licenses/fonts.txt`, and release archives carry them in
   `THIRD_PARTY_LICENSES`.

The build is static files with the fonts alongside, so the node can embed and
serve it with no outside requests.

## Node client

`@minanode/node-client` reads a Mina Rust node. Its main entry, which needs no
framework, has `createNodeClient({ baseUrl })` with a typed method per endpoint:
`/status`, `/build_env`, `/stats/sync`, `/stats/block_producer`,
`/snarker/control` (read, and change through `changeWorker`),
`/snarker/included`, `/transaction-pool`, `/state/peers`, and GraphQL's
`bestChain`, `block` and `account`. The answers are parsed into plain types.
Failures throw a `NodeError` whose `kind` says what happened: `offline` (no
answer), `absent` (an older build, or the route compiled out), `refused` (a
feature turned off, or a bad input), `failed`, or `invalid`. The rules and words
for sync, restore, the worker and the producer are there too, so every app
describes the node the same way.

`@minanode/node-client/react` adds `NodeClientProvider`, a shared
`NodeStatusProvider`, and a hook per endpoint (`usePeers()`,
`useWorkerControl()`, `useBlock(id)`, ...). Each hook returns
`{ data, error, updatedAt, loading, refresh }`. Polling pauses while the tab is
hidden and keeps the last answer when a request fails.

To use it from an app:

1. Add `"@minanode/node-client": "0.1.0"` to its `dependencies`.
2. Wrap the app: `<NodeClientProvider client={createNodeClient({ baseUrl })}>`.
   The dashboard passes no `baseUrl`, so it reads the node that serves it; an
   app elsewhere passes the node's address. Add `<NodeStatusProvider>` if
   several components show the status.
3. Read with the hooks, and show the age with the design system's `DataAge`.

[`node-client/README.md`](node-client/README.md) lists the whole API. Its tests
run the parsers against fixtures recorded from devnet nodes.

## Dashboard

`mina node` serves the dashboard on its HTTP port (`--no-dashboard` turns it
off). Build it before the node: `make dashboard` at the repository root runs
`npm ci` and `npm run build:dashboard` here, and cargo embeds `dashboard/dist`
(the `dashboard` feature of `mina-node-native`, on by default for the `mina`
CLI). A node built without it serves a page saying so; cargo never runs npm.
`tools/release/build.sh` builds it first and records it in `build-info.json`.

The node serves `index.html` for the dashboard's own pages only, which
`dashboard/src/routes.json` lists for both the app's router and the node, and
every API route wins over them. The app calls the node's API on its own origin,
through `@minanode/node-client`.

The dashboard's own code is its routes (`src/app/router.tsx`, `routes.json`),
its shell with the node's navigation and status (`src/app/shell.tsx`), the
node's wording for a failed request (`src/components/states.tsx`), the docs
links (`src/lib/docs.ts`), and a file per page that composes design-system
patterns with node-client hooks. Its pages hold only what is particular to one
page: the worker's start and stop form, the overview's restore line, and the
tables' columns.

For development, run a node, then
`MINA_NODE=http://127.0.0.1:3700 npm run dev:dashboard`: Vite serves the app on
4282 and forwards the API paths to the node. With `MINA_NODE_READONLY=1` it
refuses everything but GET and HEAD before it leaves the machine, for pointing
at a node you must not change (GraphQL pages then fail, as GraphQL queries are
POSTs).

The pages: overview (sync phase, tips, peers, restore, roles, build), SNARK
worker (state, start, stop and re-price through `POST /snarker/control`, the
included-work log), block producer (read-only), blocks and block detail, account
lookup, mempool and peers, and a GraphQL page with the endpoint, an example to
copy and links to the API docs (the node serves no GraphiQL by default: its page
loads scripts from a CDN), a status page with the node's raw `/status` or
`/build_env` as a JSON tree, and a Report a problem page: `mina report`'s
redacted bug report, prepared on a click through `POST /report`, to copy,
download or open as a pre-filled GitHub issue (the overview also says when the
node crashed lately, from `POST /report/crash`).

## Web node page

`minaweb/` is the page for minaweb.org: a Mina node that runs in the browser
tab. It downloads the node's WASM, checks its SHA-256 against the one compiled
into the page, starts it, and follows it to Synced. The boot screen shows six
steps: check the browser, download, start, connect to a peer, sync the ledgers,
sync the blocks. A failure names its cause: a page that isn't cross-origin
isolated (not HTTPS or localhost, or served without the COOP and COEP headers),
a browser without WebAssembly threads, a missing file, a WASM that fails its
integrity check, no peer reachable.

Once the node runs, the pages read it through `@minanode/node-client`'s web node
transport (`createWebNodeTransport`), which answers the HTTP API's routes from
the WASM's `RpcSender`. So the hooks and parsers are the dashboard's, and so is
the Peers page, which the app imports from `minanode-dashboard`. Its own pages
are the overview (sync, best tip, peers, memory, block checks) and blocks (the
blocks the node received, with the checks it ran on each).

On devnet, and only there, it also has a key and two roles
(`minaweb/src/roles`):

- **Your key**: a key made in the browser (the WASM's CSPRNG), or imported
  behind a warning that the same key controls the same mainnet account. It stays
  in the tab's memory. It is stored only encrypted with the visitor's passphrase
  (PBKDF2-SHA-256, AES-256-GCM): in localStorage if they ask, and in
  sessionStorage for the block producer's restart.
- **Send**: a payment signed in the WASM and submitted to the node's pool,
  followed until a block includes it.
- **SNARK worker** and **block producer**: each shows what it costs the device
  before Start. A node runs one role, fixed when it starts (`run`'s arguments),
  so starting, stopping or switching one starts the node again: the page reloads
  and its store brings the chain back. Running, they show the dashboard's worker
  and producer views. The producer screen reads the key's stake in both epoch
  ledgers and says when stake delegated today counts.

Every role keeps the privacy defaults: no peer discovery, the build's seed
relays only.

It is its own app, not a second entry of the dashboard: it owns the node's
lifecycle (one node per tab, started once per page load; only a role change
reloads the page) and is served by a static host with COOP and COEP, while the
dashboard is embedded in `mina node` with a strict Content-Security-Policy and
the node's routes.

The web node it starts is compiled in from `MINAWEB_*` variables, listed in
`minaweb/src/boot/build-config.ts`. `tools/regression/webnode/build.sh` sets
them and assembles the page with the WASM, the circuit caches, the genesis and
the seed list. For development, point `MINAWEB_ASSETS` at the `browser`
directory of such a build: `npm run dev:minaweb` then serves its WASM and takes
the rest from its `build-info.json`. The dev server sends the isolation headers.

## Facts rule

The copy lives in `minanode-site/content/site.ts` and
`minanode-site/content/docs.ts`. Each claim has an ID, and
`minanode-site/CLAIMS.md` records its wording, figures and caveat. Measurements
from pre-release qualification are marked as private, coordinator-supplied
evidence. `npm run check:claims` checks IDs and external resources, but does not
independently validate those private measurements. The release owner reviews
them before publication.

The Docs pages (`/docs/`) are short instructions for running the node. Their
frame and body copy are site components in `minanode-site/components/docs/` (the
page with its Docs list and previous and next links, sections, inline code, an
underlined text link, a caution and a list of terms) built on the design
system's patterns; they add no design-system pattern. Commands, flags and file
names there are checked against the code and `website/docs/node-runners/`, not
the claims sources.

## Hosting notes

Not deployed. When it is: the pages need inline scripts (the theme script and
Next.js's inline page data), so a Content-Security-Policy needs
`script-src 'self' 'unsafe-inline'` or their hashes; everything else can be
`'self'`. Serve `og.png` and `apple-touch-icon.png` as `image/png`.
