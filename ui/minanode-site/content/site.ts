// Every number and claim on minanode.org lives in this file, tagged with a
// claim id. CLAIMS.md maps each id to its source line in
// the private qualification notes, the private qualification notes or a research note, with
// the few words of caveat that have to stay with it. Elements that render a
// claim carry `data-claim` (space-separated when one element holds several),
// so the page can be checked against CLAIMS.md (scripts/check-claims.mjs).
//
// Keep the inline caveats. Do not add a number here without a source line.

import { release, releaseLabel } from "./release";

import type { Status } from "@minanode/design-system/patterns/status-badge";

/** Public explorer link for a devnet block. The explorer is a public,
 * read-only site; linking it makes no request until a visitor clicks. */
export function devnetBlock(height: number) {
  return `https://o1-labs.github.io/mina-explorer/#/block/${height}?network=devnet`;
}

export const nav = [
  { href: "/", label: "Home" },
  { href: "/docs/", label: "Docs" },
  { href: release.repositoryUrl, label: "GitHub" },
];

export const hero = {
  status: { claim: "C04", label: releaseLabel },
  title: {
    claim: "C02",
    lead: "Client diversity for Mina.",
    accent: ["A native node in Rust.", "A full node in the browser."],
  },
  lead: {
    claim: "C03",
    text: "A community-led second Mina client, checked against OCaml 4.0.2 by replaying the chain, checking proofs and comparing fork choice.",
  },
};

export type Metric = {
  claim: string;
  value: string;
  label: string;
  detail: string;
  href?: string;
  hrefLabel?: string;
};

/** The four proof points under the hero: rounded bounds the release
 * candidate meets, so they stay true between measurements. */
export const proofPoints: Metric[] = [
  {
    claim: "C84",
    value: "About 1 min",
    label: "Devnet sync from scratch",
    detail: "From OCaml peers; about 2 min with 12 nodes starting at once.",
  },
  {
    claim: "C82",
    value: "Under 3 min",
    label: "Mainnet sync from scratch",
    detail: "From an empty store, via OCaml peers.",
  },
  {
    claim: "C30",
    value: "Under 20 s",
    label: "Restart to synced",
    detail: "The node replays its saved chain.",
  },
  {
    claim: "C85 C86",
    value: "About 1 GB",
    label: "RAM for a devnet full node",
    detail: "Typical use. A Raspberry Pi 4 syncs in under 5 min.",
  },
];

export type Role = {
  id: string;
  claim: string;
  title: string;
  status: Status;
  statusLabel: string;
  text: string;
  link?: { height: number };
};

export const about = {
  claim: "C09",
  title: "A second, independent Mina client",
  lead: "Mina has one working node implementation, OCaml. With a second, a bug in one no longer has to be a bug in every node.",
};

export const roles: Role[] = [
  {
    id: "full-node",
    claim: "C13",
    title: "Full node",
    status: "live",
    statusLabel: "Working",
    text: "Syncs mainnet and devnet from OCaml peers and follows the tip. 46 of 46 sampled tips matched public Auro.",
  },
  {
    id: "block-producer",
    claim: "C15 C17",
    title: "Block producer",
    status: "devnet",
    statusLabel: "Public devnet",
    text: "Its blocks reach devnet's best chain, from a shared producer key. Not yet on mainnet.",
    link: { height: 579510 },
  },
  {
    id: "snark-worker",
    claim: "C18",
    title: "SNARK worker",
    status: "devnet",
    statusLabel: "Public devnet",
    text: "Proves the transaction SNARK work that block producers buy. Devnet producers have bought its bundles.",
  },
  {
    id: "web-node",
    claim: "C11 C83",
    title: "Web node",
    status: "pending",
    statusLabel: "Coming soon",
    text: "The full node in a browser tab, with nothing to install. It follows this native release shortly.",
  },
];

export type Build = {
  id: string;
  eyebrow: string;
  title: string;
  body: { claim: string; text: string };
  metrics: Metric[];
};

export const builds: Build[] = [
  {
    id: "native",
    eyebrow: "Native",
    title: "A Rust node",
    body: {
      claim: "C32",
      text: "A native daemon for every role, with a GraphQL API and a feed to OCaml's archive.",
    },
    metrics: [
      {
        claim: "C05",
        value: "15,897",
        label: "Mesa mainnet blocks replayed",
        detail: "Heights 548148–564044; every staged-ledger hash matched OCaml.",
      },
      {
        claim: "C08",
        value: "121",
        label: "SNARK bundles bought",
        detail: "From its worker, by seven devnet producers in one test.",
      },
      {
        claim: "C40",
        value: "5m55.84s",
        label: "OCaml syncs from it",
        detail: "A fresh OCaml devnet node, the Rust node its only peer; one run.",
      },
    ],
  },
  {
    id: "web",
    eyebrow: "WebAssembly",
    title: "A full node in a browser tab",
    body: {
      claim: "C11 C21",
      text: "The same Rust code, compiled to WebAssembly. It syncs mainnet, verifies proofs, and signs and sends transactions over WebRTC.",
    },
    metrics: [
      {
        claim: "C24",
        value: "3.1 MB",
        label: "Download",
        detail: "WASM, Brotli-compressed.",
      },
      {
        claim: "C25",
        value: "1,171 MiB",
        label: "Peak WASM memory",
        detail: "Median, linear memory only.",
      },
      {
        claim: "C26 C07",
        value: "42.8 s",
        label: "Block proof",
        detail: "For canonical devnet block 578136, under load.",
      },
    ],
  },
];

export const getStarted = {
  claim: "C50",
  title: `Run ${releaseLabel}`,
  lead: "Download the archive for your CPU, run the node and open its dashboard.",
  items: [
    {
      claim: "C50 C56",
      title: "Native node",
      status: "devnet" as Status,
      statusLabel: "Available",
      body: "Linux builds for x86-64 and ARM64, with the devnet prover circuits inside.",
    },
    {
      claim: "C50 C83",
      title: "Web node",
      status: "pending" as Status,
      statusLabel: "Coming soon",
      body: "The browser node is a separate build.",
    },
    {
      claim: "C50",
      title: "Source code",
      status: "live" as Status,
      statusLabel: "Source",
      body: "Build the node and dashboard from the public source.",
    },
  ],
};
