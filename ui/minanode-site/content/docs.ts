import { archiveName, releaseLabel } from "./release";

// The Docs pages' numbers and claims, as content/site.ts holds the home
// page's: each with a claim id that CLAIMS.md maps to its source lines, and
// rendered by an element carrying `data-claim`. The pages' other words are
// instructions (commands, flags, where things are), checked against the code
// and website/docs/node-runners/ rather than the claims sources.
//
// Do not add a number here without a source line.

export type Claimed = { claim: string; text: string };

export type DocsPage = {
  slug: string;
  title: string;
  /** One line: the index card and the page's meta description. */
  summary: string;
  /** Left out of the navigation, the index and the sitemap. Its route folder
   * is private (`app/docs/_<slug>/`), so the page isn't built either. */
  hidden?: boolean;
};

/** Every Docs page, in reading order, hidden ones included. */
const allDocsPages: DocsPage[] = [
  {
    slug: "quickstart",
    title: "Quickstart",
    summary: "Download, run and open the dashboard.",
  },
  {
    slug: "requirements",
    title: "Requirements",
    summary: "The machine each role needs.",
  },
  {
    slug: "install",
    title: "Install",
    summary: "Download the release, check it and unpack it.",
  },
  {
    slug: "docker",
    title: "Run with Docker",
    summary: "Run the node in a container on Linux, macOS or Windows.",
  },
  {
    slug: "build-from-source",
    title: "Build from source",
    summary: "Build the dashboard and node from the GitHub source.",
  },
  {
    slug: "run-a-node",
    title: "Run a node",
    summary: "Start the node; its data, ports and logs.",
  },
  {
    slug: "run-a-relay",
    title: "Run a relay",
    summary:
      "Accept native and browser peers while keeping the dashboard private.",
    // Hidden until the web node launches: a relay serves browser nodes.
    hidden: true,
  },
  {
    slug: "dashboard",
    title: "The dashboard",
    summary: "What each page shows, and how to open it on a server.",
  },
  {
    slug: "snark-worker",
    title: "SNARK worker",
    summary: "Prove SNARK work for fees, from the dashboard or with flags.",
  },
  {
    slug: "block-producer",
    title: "Block producer",
    summary: "Produce blocks with your key.",
  },
  {
    slug: "light-endpoints",
    title: "Light client endpoints",
    summary: "Read the best tip and account Merkle paths, and verify them.",
  },
  {
    slug: "bug-reports",
    title: "Reporting a bug",
    summary: "Collect a report from the command line or the dashboard.",
  },
  {
    slug: "cli",
    title: "CLI reference",
    summary: "Every command and option, generated from the binary.",
  },
];

/** The Docs pages shown, in reading order. */
export const docsPages: DocsPage[] = allDocsPages.filter(page => !page.hidden);

export const docsIndex = {
  status: { claim: "C04", label: releaseLabel },
  beta: {
    claim: "C04",
    text: "0.20.0 is a beta release. Run it on devnet and in test setups. Expect changes before it is ready for production.",
  },
  lead: {
    claim: "C60",
    text: "minanode 0.20.0 is a Mina node for the public devnet: one program for Linux on x86-64 or ARM64, with a dashboard. It runs as a full node, verifying the chain, and may also prove SNARK work or produce blocks.",
  },
  limitations: {
    claim: "C76",
    items: [
      "Devnet only.",
      "Hard forks aren't handled yet: each one will need a new release.",
      "Memory rises slowly on a node that runs for days.",
      "The browser node is a separate build from this native release.",
    ],
  },
};

export const requirements = {
  system: [
    {
      claim: "C60",
      label: "Operating system",
      value: "Linux, x86-64 or ARM64",
    },
    { claim: "C60", label: "C library", value: "glibc 2.31 or newer" },
    {
      claim: "C61",
      label: "x86-64 CPU",
      value: "Intel Broadwell or AMD Zen 1, or newer",
    },
    { claim: "C63", label: "Clock", value: "Kept in sync by NTP" },
    {
      claim: "C62",
      label: "Other software",
      value: "None: the devnet circuits and the dashboard are inside mina",
    },
    {
      claim: "C68",
      label: "Disk",
      value: "5 GB free",
    },
  ],
  cpu: {
    claim: "C61",
    text: "mina uses two sets of x86-64 instructions, BMI2 and ADX. On an older CPU that lacks either, it stops with an illegal instruction error. This prints both names on a CPU that has them:",
  },
  memory: [
    {
      claim: "C64",
      label: "Devnet full node, x86_64",
      value: "Typical 1.2 GB; peak 1.5 GB over 10 hours",
    },
    {
      claim: "C64",
      label: "Mainnet full node, x86_64",
      value: "Typical 1.3 GB; peak 2.0 GB over 24 hours, on an older build",
    },
    {
      claim: "C67",
      label: "Devnet block producer",
      value:
        "Typical 2.1 GB; peak 3.4 GB over 10 hours, including the block prover",
    },
    {
      claim: "C66",
      label: "Devnet node with SNARK worker",
      value: "Typical 3.2 GB; peak 3.7 GB over 10 hours",
    },
    {
      claim: "C65",
      label: "Raspberry Pi 4, 4 GB, ARM64",
      value:
        "Devnet full node, two 3-hour runs: steady mean 341 and 344 MiB, steady p95 364 and 368 MiB; peak 570 and 652 MiB while catching up",
    },
    {
      claim: "C89",
      label: "Apple M4, Docker, ARM64",
      value:
        "Devnet full node over 10 minutes synced: median 322 MiB, peak 370 MiB. Block producer: 1.14 to 1.20 GiB. SNARK worker: 1.05 GiB, 1.80 GiB once it loads the zkApp provers",
    },
  ],
  machine: [
    { claim: "C64", label: "Full node", value: "2 cores, 4 GB RAM" },
    { claim: "C66", label: "SNARK worker", value: "4 or more cores, 8 GB RAM" },
    {
      claim: "C67",
      label: "Block producer",
      value: "4 or more cores, 8 GB RAM",
    },
  ],
  memoryNote: {
    claim: "C64 C65 C66 C67 C89",
    text: "The x86_64 rows are RSS sampled once a minute on 24 October 2026, in rounded GB, where 1 GB is 1,000,000,000 bytes. Typical means the median; peak means the highest sample. The ARM64 rows are shorter runs. The Pi 4 rows give each run's mean and 95th percentile of RSS sampled every 30 seconds, from 30 minutes after sync to the end of the run. The Apple M4 rows are docker stats samples taken every 10 seconds, the producer and worker ones as the range seen. A block producer must also prove each block within its 90-second slot, so a faster CPU gives it more margin.",
  },
  disk: {
    claim: "C68",
    text: "A synced devnet node's data measured 237 MB: 132 MB of saved chain state, 70 MB of ledger accounts and 29 MB of proof cache. Logs take up to 200 MB more. 5 GB free leaves room for growth and upgrades.",
  },
};

export const install = {
  pathNote:
    "Use the full path for the first check. A fresh shell may not have ~/.local/bin on PATH yet; add it to PATH to run mina by name.",
  archives: [
    {
      claim: "C69",
      label: "x86-64 (uname -m says x86_64)",
      value: archiveName("x86_64"),
    },
    {
      claim: "C69",
      label: "ARM64 (uname -m says aarch64)",
      value: archiveName("arm64"),
    },
  ],
};

export const sourceBuild = {
  versions: {
    claim: "C77",
    text: "The repository pins Rust 1.92 and Node.js 24.11.0. A native build needs no nightly toolchain.",
  },
  circuits: {
    claim: "C78",
    text: "The devnet prover circuits are in the source and embedded in the default build. There is no separate circuit download.",
  },
  measurements: {
    claim: "C79",
    text: "In one clean-source build on a 16-core x86_64 machine, peak sampled build RAM was 4.93 GiB, and the target directory plus toolchains took 4.57 GiB.",
  },
  release: {
    claim: "C80",
    text: "Release archives use tools/release/build.sh with cargo-zigbuild and a glibc 2.31 floor.",
  },
};

export const dashboard = {
  remoteNote:
    "The HTTP API and dashboard listen on 127.0.0.1 by default. For a private network such as Tailscale, start the node with --http-addr followed by its private IP address. --http-addr 0.0.0.0 listens on every IPv4 interface.",
  tunnelNote:
    "Your SSH account must allow TCP forwarding. If the tunnel connects but the page fails to load, check the SSH client's error for a forwarding refusal and ask the server administrator to allow it for your account.",
  tailscaleNote:
    "Use the Tailscale address for worker control only if your tailnet rules limit access to people you trust with the worker's fee receiver.",
};

export const bugReports = {
  sharingNote:
    "Copy only the report body after reviewing it. Collection progress printed before the report can show your work directory's full path, including your user name. The report body can show a loopback address; peer IP addresses and peer ids are omitted by default.",
};

export const run = {
  /** The work directory's default, ~/.mina: the sentence is on the page. */
  workDir: { claim: "C70" },
  ports: [
    {
      claim: "C70",
      label: "3000 (--port)",
      value: "HTTP: the API and dashboard.",
    },
    {
      claim: "C70",
      label: "8302 (--libp2p-port)",
      value: "Peers. Open it to let peers reach your node.",
    },
  ],
  logs: {
    claim: "C71",
    text: "The node logs to the terminal and to mina.log in its work directory. The log rotates at 20 MB and 10 files are kept, so logs take at most 200 MB.",
  },
};

export const worker = {
  fee: {
    claim: "C72",
    text: "The fee is in MINA, and the default is 0.1.",
  },
  selection: {
    claim: "C72",
    text: "By default it picks one of those jobs at random, so workers spread over the jobs rather than racing for the same one.",
  },
  feeRule: {
    claim: "C73",
    text: "If your key has no account on chain yet, nodes accept your work only with a fee of at least 1 MINA, the account creation fee: a block that bought it would pay the fee to a new account, which costs 1 MINA. Fund the account first, or ask at least 1 MINA. The dashboard refuses a lower fee for such a key. With --run-snarker the node starts, and logs a warning once the first proof is refused.",
  },
  confirmed: {
    claim: "C74",
    text: "Work is pending until 15 blocks are on top of its block, Mina's recommended wait for a payment, and confirmed from then on. Dropped means another block took its place.",
  },
};

export const producer = {
  slotChecks: {
    claim: "C75",
    text: "The files hold no private key, but they do reveal your future winning slots, so the node makes them readable by you only (mode 0600).",
  },
};

export const lightEndpoints = {
  bestTipSize: {
    claim: "C85",
    text: "The answer is 23,546 bytes in binprot and 32,344 bytes in JSON.",
  },
  accountSize: {
    claim: "C86",
    text: "An answer for an account without a zkApp is about 4.8 KB.",
  },
  limits: [
    {
      claim: "C87",
      label: "Requests in flight",
      value:
        "16, for /light/* and the GraphQL path fields together. More are refused at once with busy: there is no queue.",
    },
    {
      claim: "C87",
      label: "Accounts pending",
      value:
        "32, over all pending light requests. A request that would take more is refused with busy.",
    },
    {
      claim: "C87",
      label: "Accounts per membership query",
      value:
        "32. More get the GraphQL error at most 32 accountInfos per request.",
    },
    {
      claim: "C87",
      label: "Wait for the node",
      value: "10 seconds, then timeout.",
    },
  ],
  nextWindow: {
    claim: "C88",
    text: "On 26 October 2026, the 290 blocks between root and tip spanned 580 slots of 90 seconds, so next was unavailable for about 14.5 hours after the epoch began. If every slot had a block, the gap would be about 7.25 hours.",
  },
};
