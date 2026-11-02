import { archiveName, release, releaseLabel } from "./release";

// The Docs pages' numbers and claims, as content/site.ts holds the home
// page's: each with a claim id that CLAIMS.md maps to its source lines, and
// rendered by an element carrying `data-claim`. The pages' other words are
// instructions (commands, flags, where things are), checked against the code
// and website/docs/node-runners/ rather than the claims sources.
//
// Do not add a number here without a source line.

export type Claimed = { claim: string; text: string };

/** The navigation's groups, in reading order. */
export const docsGroups = [
  { id: "start", title: "Get started" },
  { id: "native", title: "Native node" },
  { id: "roles", title: "Node roles" },
  { id: "web", title: "Web node" },
  { id: "reference", title: "Reference and help" },
] as const;

export type DocsGroupId = (typeof docsGroups)[number]["id"];

export type DocsPage = {
  slug: string;
  title: string;
  /** One line: the index card and the page's meta description. */
  summary: string;
  group: DocsGroupId;
  /** Left out of the navigation, the index and the sitemap. Its route folder
   * is private (`app/docs/_<slug>/`), so the page isn't built either. */
  hidden?: boolean;
};

/** The Docs home page: first in the Get started group. */
export const docsOverview: DocsPage = {
  slug: "",
  title: "Overview",
  summary: "What the node is, what it runs on, and its known limitations.",
  group: "start",
};

/** Every Docs page, in reading order within its group, hidden ones
 * included. */
const allDocsPages: DocsPage[] = [
  {
    slug: "quickstart",
    title: "Quickstart",
    summary: "Download the node, run it, and open the dashboard.",
    group: "start",
  },
  {
    slug: "requirements",
    title: "Requirements",
    summary: "The machine, memory, and disk each role needs.",
    group: "start",
  },
  {
    slug: "install",
    title: "Install",
    summary: "Download the release, verify it, and install it.",
    group: "native",
  },
  {
    slug: "run-a-node",
    title: "Run a node",
    summary: "Start the node, keep it running, and find its data and logs.",
    group: "native",
  },
  {
    slug: "docker",
    title: "Run with Docker",
    summary: "Run the node in a container on Linux, macOS, or Windows.",
    group: "native",
  },
  {
    slug: "dashboard",
    title: "The dashboard",
    summary: "What each dashboard page shows, and how to open it on a server.",
    group: "native",
  },
  {
    slug: "build-from-source",
    title: "Build from source",
    summary: "Build the dashboard and the node from the GitHub source.",
    group: "native",
  },
  {
    slug: "snark-worker",
    title: "Run a SNARK worker",
    summary: "Prove SNARK work for fees, from the dashboard or with flags.",
    group: "roles",
  },
  {
    slug: "block-producer",
    title: "Produce blocks",
    summary: "Produce blocks with your key, and follow each won slot.",
    group: "roles",
  },
  {
    slug: "web-node-connections",
    title: "How the web node works",
    summary:
      "Relays and routes, how it syncs, the saved chain, one role per tab, and sharing between browsers.",
    group: "web",
  },
  {
    slug: "run-web-node",
    title: "Run the web node yourself",
    summary: "Download, verify, and run the web node on your computer.",
    group: "web",
  },
  {
    slug: "run-a-relay",
    title: "Run a relay",
    summary:
      "Accept native and web node peers, and keep the dashboard private.",
    group: "web",
  },
  {
    slug: "cli",
    title: "CLI reference",
    summary: "Every command and option, generated from the binary.",
    group: "reference",
  },
  {
    slug: "light-endpoints",
    title: "Light client endpoints",
    summary: "Read the best tip and account Merkle paths, and verify them.",
    group: "reference",
  },
  {
    slug: "bug-reports",
    title: "Report a bug",
    summary: "Collect a report from the command line or the dashboard.",
    group: "reference",
  },
];

/** The Docs pages shown, in reading order: group by group. */
export const docsPages: DocsPage[] = docsGroups.flatMap(group =>
  allDocsPages.filter(page => page.group === group.id && !page.hidden),
);

/** The groups with their shown pages, the overview first in its group. */
export const docsNav = docsGroups.map(group => ({
  ...group,
  pages: [
    ...(group.id === docsOverview.group ? [docsOverview] : []),
    ...docsPages.filter(page => page.group === group.id),
  ],
}));

export const docsIndex = {
  status: { claim: "C04", label: releaseLabel },
  beta: {
    claim: "C04",
    text: `${release.version} is a beta release. Run it on devnet and in test setups. Expect changes before it is ready for production.`,
  },
  lead: {
    claim: "C60",
    text: `minanode ${release.version} is a Mina node for the public devnet. It is one program for Linux on x86-64 or ARM64, with a dashboard. It runs as a full node that verifies the chain, and it can also prove SNARK work or produce blocks.`,
  },
  limitations: {
    claim: "C76",
    items: [
      "Devnet only.",
      "Hard forks aren't handled yet. Each one will need a new release.",
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
      value:
        "None at run time. The devnet circuits and the dashboard are inside mina. Installing needs the tools below.",
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
      value: "About 360 to 380 MiB over 9 hours. Peak 644 MiB.",
    },
    {
      claim: "C64",
      label: "Mainnet full node, x86_64",
      value: "About 1.2 GiB over 9 hours. Peak 2.6 GiB.",
    },
    {
      claim: "C67",
      label: "Devnet block producer",
      value:
        "About 1.3 GiB over 9 hours. Peak 2.7 GiB, including the block prover.",
    },
    {
      claim: "C66",
      label: "Devnet node with SNARK worker",
      value: "About 2.0 to 2.2 GiB over 9 hours. Peak 2.6 GiB.",
    },
    {
      claim: "C65",
      label: "Raspberry Pi 4, 4 GB, ARM64",
      value:
        "Devnet full node, two 3-hour runs. Steady mean 341 and 344 MiB, steady 95th percentile 364 and 368 MiB. Peak 570 and 652 MiB while catching up.",
    },
    {
      claim: "C89",
      label: "Apple M4, Docker, ARM64",
      value:
        "Devnet full node over 10 minutes synced: median 322 MiB, peak 370 MiB. Block producer: 1.14 to 1.20 GiB. SNARK worker: 1.05 GiB, and 1.80 GiB once it loads the zkApp provers.",
    },
  ],
  machine: [
    {
      claim: "C64",
      label: "Full node",
      value: "1 core and 2 GiB RAM to try. 2 cores and 4 GiB suggested.",
    },
    {
      claim: "C66",
      label: "SNARK worker",
      value: "4 cores and 8 GiB RAM to try. 8 cores and 16 GiB suggested.",
    },
    {
      claim: "C67",
      label: "Block producer",
      value: "4 cores and 8 GiB RAM to try. 8 cores and 16 GiB suggested.",
    },
  ],
  memoryNote: {
    claim: "C64 C65 C66 C67 C89",
    text: "The x86_64 rows come from 0.20.0 nodes on 27 and 28 October 2026. Each gives the range of RSS over 9 hours of running, and the peak. The peak is the process's high-water mark since it started, so it includes restoring the saved chain. The ARM64 rows come from shorter runs. The Pi 4 rows give each run's mean and 95th percentile of RSS, sampled every 30 seconds from 30 minutes after sync to the end of the run. The Apple M4 rows come from docker stats samples taken every 10 seconds, and give the range seen for the producer and the worker. A block producer must also prove each block within its 90-second slot, so a faster CPU gives it more margin.",
  },
  disk: {
    claim: "C68",
    text: "A synced devnet node's data measured 237 MB: 132 MB of saved chain state, 70 MB of ledger accounts and 29 MB of proof cache. Logs take up to 200 MiB more. 5 GB free leaves room for growth and upgrades.",
  },
};

export const install = {
  pathNote:
    "Use the full path for the first check, because a fresh shell may not have ~/.local/bin on PATH yet. To run mina by name, add ~/.local/bin to PATH.",
  archives: [
    {
      claim: "C69",
      label: "x86-64, where uname -m prints x86_64",
      value: archiveName("x86_64"),
    },
    {
      claim: "C69",
      label: "ARM64, where uname -m prints aarch64",
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
    text: "The 0.20.1 source embeds the devnet and mainnet prover circuits in the default native build. Each file is checked against its pinned SHA-256 before use. There is no separate circuit download.",
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

/** The dashboard's SSH tunnel, explained in the dashboard page's "Open it on a
 * server". */
export const sshTunnel =
  "ssh -N -o ExitOnForwardFailure=yes -L 3000:127.0.0.1:3000 you@server";

export const dashboard = {
  remoteNote:
    "The HTTP API and the dashboard listen on 127.0.0.1 by default. To reach them over a private network such as Tailscale, start the node with --http-addr and the machine's private IP address. --http-addr 0.0.0.0 listens on every IPv4 interface.",
  tunnelNote:
    "The tunnel needs an SSH account that allows TCP forwarding. If the tunnel connects but the page does not load, look for a forwarding refusal in the SSH client's errors. To fix it, ask the server administrator to allow forwarding for your account.",
  tailscaleNote:
    "Use worker control through the Tailscale address only if your tailnet rules limit access to people you trust with the worker's fee receiver.",
};

export const bugReports = {
  sharingNote:
    "Review the report, then copy only the report body. The progress lines printed before the report can show your work directory's full path, including your user name. The report body can show a loopback address. It leaves out peer IP addresses and peer ids by default.",
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
      value: "Peers. Open it to let other nodes reach yours.",
    },
  ],
  logs: {
    claim: "C71",
    text: "The node logs to the terminal and to mina.log in its work directory. The log rotates at 20 MiB and 10 files are kept, so logs take at most 200 MiB.",
  },
};

export const worker = {
  mainnet: {
    claim: "C94",
    text: "The 0.20.1 mainnet worker passed offline base, merge and zkApp proof tests with embedded circuits. Public mainnet work publication, purchases and fees remain untested.",
  },
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
    text: "If your key has no account on chain yet, nodes accept your work only with a fee of at least 1 MINA, the account creation fee. A block that bought the work would pay the fee to a new account, and creating that account costs 1 MINA. Fund the account first, or ask at least 1 MINA. The dashboard refuses a lower fee for such a key. With --run-snarker, the node starts, and logs a warning when the first proof is refused.",
  },
  confirmed: {
    claim: "C74",
    text: "Work is pending until 15 blocks are on top of its block, and confirmed from then on. 15 blocks is Mina's recommended wait for a payment. Dropped means another block took its place.",
  },
};

export const producer = {
  slotChecks: {
    claim: "C75",
    text: "The files hold no private key, but they reveal your future winning slots, so the node makes them readable by you only, with mode 0600.",
  },
};

export const lightEndpoints = {
  bestTipSize: {
    claim: "C90",
    text: "The answer is 23,546 bytes in binprot and 32,344 bytes in JSON.",
  },
  accountSize: {
    claim: "C91",
    text: "An answer for an account without a zkApp is about 4.8 KB.",
  },
  limits: [
    {
      claim: "C87",
      label: "Requests in flight",
      value:
        "16, for /light/* and the GraphQL path fields together. More are refused at once with busy. There is no queue.",
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

/** UPTIME-0201C measurements and 0201D release behavior. */
export const uptime = {
  reports: {
    claim: "C120",
    text: "0.20.1 can send signed blocks and selected SNARK work. Reporting is off by default. Attempts start every five slots, or 450 seconds with 90-second slots.",
  },
  memory: {
    claim: "C121",
    text: "Allow about 560 MiB extra resident memory above a producer for uptime circuits. A selected zkApp job can load about 700 MiB more. These are circuit-load measurements, not proving peaks.",
  },
  zkapp: {
    claim: "C122",
    text: "OCaml and Rust both send terminal zkApp work with a zero SOK digest. OCaml's delegation verifier rejects its binding to the submitter and fee. Local verification does not establish that the live service accepts it.",
  },
};
