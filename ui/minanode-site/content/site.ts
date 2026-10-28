// Every claim on minanode.org lives in this file, tagged with a claim id.
// CLAIMS.md maps each id to the evidence behind it, from the private
// qualification notes or a research note. Elements that render a claim carry
// `data-claim` (space-separated when one element holds several), so the page
// can be checked against CLAIMS.md (scripts/check-claims.mjs).
//
// The home page shows direction, not test results: keep its claims general
// enough to stay true between releases, with the measurements in CLAIMS.md.

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
    text: "A community-led second Mina client, checked against the OCaml node by replaying the chain, checking proofs and comparing fork choice.",
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

/** The four proof points under the hero: general statements the release
 * meets, so they stay true between measurements. */
export const proofPoints: Metric[] = [
  {
    claim: "C84",
    value: "Under a minute",
    label: "Devnet sync from scratch",
    detail: "From an empty data directory, via OCaml peers.",
  },
  {
    claim: "C82",
    value: "A few minutes",
    label: "Mainnet sync from scratch",
    detail: "Most of it is the ledger download from OCaml peers.",
  },
  {
    claim: "C30",
    value: "Seconds",
    label: "Restart to synced",
    detail: "The node replays its saved chain.",
  },
  {
    claim: "C85 C86",
    value: "Under 1 GB",
    label: "RAM for a devnet full node",
    detail: "A few hundred MB once synced. It runs on a Raspberry Pi.",
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
    text: "Syncs mainnet and devnet from OCaml peers and follows the tip.",
  },
  {
    id: "block-producer",
    claim: "C15 C17",
    title: "Block producer",
    status: "devnet",
    statusLabel: "Public devnet",
    text: "Its blocks reach devnet's best chain. Not yet on mainnet.",
    link: { height: 579510 },
  },
  {
    id: "snark-worker",
    claim: "C18",
    title: "SNARK worker",
    status: "devnet",
    statusLabel: "Public devnet",
    text: "Produces the transaction SNARK work that block producers buy. Devnet producers buy its work.",
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
        value: "Mainnet",
        label: "Replayed block by block",
        detail: "Every staged-ledger hash matched OCaml.",
      },
      {
        claim: "C93",
        value: "Archive",
        label: "Feeds OCaml's archive",
        detail: "Its archive matched one fed by OCaml on every main-chain block.",
      },
      {
        claim: "C40",
        value: "Both ways",
        label: "OCaml syncs from it",
        detail: "A fresh OCaml node syncs with the Rust node as its only peer.",
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
        value: "A few MB",
        label: "Download",
        detail: "WebAssembly, compressed.",
      },
      {
        claim: "C11",
        value: "No install",
        label: "Runs in a tab",
        detail: "Open the page and it syncs.",
      },
      {
        claim: "C26 C07",
        value: "Block producer",
        label: "From the browser",
        detail: "It produced and proved a devnet block in a tab.",
      },
    ],
  },
];

export const getStarted = {
  claim: "C50",
  title: `Run ${releaseLabel}`,
  lead: "Download the archive for your CPU or run the Docker image, then open its dashboard.",
  items: [
    {
      claim: "C50 C56",
      title: "Native node",
      status: "devnet" as Status,
      statusLabel: "Available",
      body: "Linux builds for x86-64 and ARM64, with the devnet prover circuits inside.",
    },
    {
      claim: "C50 C92",
      title: "Docker image",
      status: "devnet" as Status,
      statusLabel: "Available",
      body: "For x86-64 and ARM64. With Docker Desktop it also runs on macOS and Windows.",
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
