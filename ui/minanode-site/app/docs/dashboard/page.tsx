import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";

import { DocsPage } from "@/components/docs/docs-page";
import { Screenshot } from "@/components/docs/screenshot";
import {
  C,
  Caution,
  DocLink,
  DocSection,
  Prose,
  Terms,
} from "@/components/docs/prose";
import { dashboard, sshTunnel } from "@/content/docs";

export const metadata: Metadata = {
  title: "The dashboard",
  description:
    "The dashboard minanode serves on its HTTP port: what each page shows, and how to open it on a server through an SSH tunnel.",
  alternates: { canonical: "/docs/dashboard/" },
};

const archiveReplay = `curl -X POST http://127.0.0.1:3000/archive/resend \\
  -H 'x-mina-archive-control: 1' \\
  -H 'content-type: application/json' \\
  -d '{"state_hash": "3N..."}'`;

const archiveMetrics = `# The archive process: serve its database counts on port 3087.
mina-archive run --postgres-uri "$ARCHIVE_DATABASE_URI" \\
  --server-port 3086 --metrics-port 3087

# The node: send blocks to port 3086, read the counts from port 3087.
MINA_ARCHIVE_ADDRESS=http://127.0.0.1:3086 \\
MINA_ARCHIVE_METRICS_ADDRESS=http://127.0.0.1:3087 \\
  mina node --archive-archiver-process`;

const archiveCounts = [
  {
    term: "Highest stored",
    text: "The highest block height in the archive's blocks table, beside how far it is below the highest block the node delivered.",
  },
  {
    term: "Missing heights",
    text: "Heights with no block row, counted from the archive's highest block down by its --missing-blocks-width, 2,000 heights by default, and never below its lowest block.",
  },
  {
    term: "Blocks without a parent",
    text: "Blocks above the archive's lowest height whose parent block isn't in the database. It counts the whole database.",
  },
  {
    term: "Checked",
    text: "When the node last read the counts. It reads them every 60 seconds.",
  },
];

const archiveVerdicts = [
  {
    term: "Complete",
    text: "No missing heights, no blocks without a parent, and the highest stored block has caught up with every block the node delivered 6 minutes earlier.",
  },
  {
    term: "Incomplete",
    text: "At least one missing height or block without a parent.",
  },
  {
    term: "Behind",
    text: "A block the archive acknowledged 6 minutes ago or more is above the highest stored block. The archive took it but didn't write it, or its counts stopped refreshing.",
  },
  {
    term: "Not checked",
    text: "MINA_ARCHIVE_METRICS_ADDRESS isn't set.",
  },
  {
    term: "Unavailable",
    text: "The last read failed, or the page at that address has no archive counts. The card keeps the last counts it read, with their time.",
  },
];

const archiveStates = [
  {
    term: "Up to date",
    text: "No diff waits, no delivery failed, and the newest delivered block is the highest block the node accepted.",
  },
  {
    term: "Catching up",
    text: "Diffs wait in the outbox. The count says how many.",
  },
  {
    term: "Delivery failing",
    text: "The archive refused the diff or didn't answer. The node retries the same block with back-off. Waiting diffs stay in the outbox up to its size limits.",
  },
  {
    term: "Node not synced",
    text: "The node is still syncing, so its feed can't be up to date with the network.",
  },
  {
    term: "Database incomplete",
    text: "The archive database check found missing heights or blocks without a parent.",
  },
  {
    term: "Database behind",
    text: "The archive's highest stored block is below blocks the node delivered 6 minutes ago or more.",
  },
];

const pages = [
  {
    label: "Overview",
    value:
      "Sync state, best tip, peers, the saved-state restore, and the node's roles",
  },
  {
    label: "SNARK worker",
    value:
      "The worker's state, its start, stop, and fee, and the work that blocks bought",
  },
  {
    label: "Block producer",
    value:
      "The producer key, the won slots ahead, what became of each won slot and its stages, and the totals",
  },
  {
    label: "Blocks",
    value:
      "The newest blocks of the best chain, and each block's transactions and SNARK work",
  },
  {
    label: "Accounts",
    value: "An account's balance and nonce at the best tip",
  },
  {
    label: "Mempool",
    value: "Transactions waiting for a block, and the SNARK work on offer",
  },
  {
    label: "Peers",
    value: "The nodes it knows, and the best tip each connected peer last sent",
  },
  { label: "Status", value: "The node's own /status and /build_env answers" },
  { label: "GraphQL", value: "The GraphQL address, with an example to copy" },
  {
    label: "Archive",
    value:
      "On a node that feeds an archive, whether the feed is up to date, the archive database's own counts, gaps, and replay of retained blocks",
  },
  {
    label: "Report a problem",
    value: "A bug report to copy, download, or open as a GitHub issue",
  },
];

export default function DashboardPage() {
  return (
    <DocsPage
      slug="dashboard"
      title="The dashboard"
      lead={
        <>
          The node serves its own dashboard on its HTTP port. On the machine
          that runs the node, open <C>http://127.0.0.1:3000</C>.
        </>
      }
    >
      <DocSection id="pages" title="What each page shows">
        <Terms
          items={pages.map(({ label, value }) => ({
            term: label,
            text: value,
          }))}
        />
        <Prose>
          <p>
            Use <C>127.0.0.1</C> rather than <C>localhost</C>. The node listens
            on IPv4 only, and a browser can try <C>localhost</C> as IPv6{" "}
            <C>::1</C> first. If another program listens on port 3000 there, it
            answers in the node&apos;s place, and neither shows an error.
          </p>
          <p>
            The pages read only this node, and nothing loads from elsewhere.{" "}
            <C>--no-dashboard</C> turns the dashboard off. The API stays on.
          </p>
          <p>
            While the worker is off, the SNARK worker page&apos;s fee field
            starts at the lowest fee on offer in the pool, or at 0.1 MINA if
            there is none. A key with no account on chain needs a fee of at
            least 1 MINA, so raise the fee for a new key.{" "}
            <DocLink href="/docs/snark-worker/#fee-rule">The fee rule</DocLink>{" "}
            explains why.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="screenshots" title="Screenshots">
        <Screenshot
          src="/images/dashboard-overview.webp"
          alt="Devnet dashboard overview with sync state, best tip, peer count and a working SNARK worker"
          caption="Overview."
          width={1440}
          height={1200}
          eager
        />
        <Screenshot
          src="/images/dashboard-snark-worker.webp"
          alt="SNARK worker dashboard with proving state, fee receiver and fee controls, and recent proof jobs"
          caption="SNARK worker."
          width={1440}
          height={1163}
        />
        <Screenshot
          src="/images/dashboard-block-producer.webp"
          alt="Active block producer dashboard with epoch progress, block outcomes and proof-time summary"
          caption="Block producer."
          width={1440}
          height={900}
        />
        <Screenshot
          src="/images/dashboard-blocks.webp"
          alt="Best-chain blocks listed by height with truncated producer keys, transaction counts, SNARK work and fees"
          caption="Blocks."
          width={1440}
          height={1155}
        />
      </DocSection>

      <DocSection id="archive-page" title="The Archive page">
        <Prose>
          <p>
            The Archive page appears on a node started with{" "}
            <C>--archive-archiver-process</C>, which sends each block it applies
            to an OCaml archive process. On any other node the page and its link
            are absent.
          </p>
          <p>
            A block is delivered once the archive process acknowledges its diff.
            The archive acknowledges a diff before it commits the block to its
            database, so delivery alone doesn&apos;t show that a block is in the
            database. The <strong>Archive database</strong> card shows the
            archive&apos;s own counts of its database, if you turn on the
            database check.
          </p>
          <p>
            The status at the top of the page is one of these. When several
            apply, it shows the first of Delivery failing, Node not synced,
            Database incomplete, Database behind and Catching up.
          </p>
        </Prose>
        <Terms items={archiveStates} />
        <Prose>
          <h3>Check the archive database</h3>
          <p>
            The OCaml archive process counts its own blocks table and serves the
            counts as Prometheus gauges on its <C>--metrics-port</C>. The node
            reads them from <C>MINA_ARCHIVE_METRICS_ADDRESS</C> every 60
            seconds, with a 5-second timeout. It never connects to PostgreSQL
            and needs no database password.
          </p>
        </Prose>
        <CodeBlock
          code={archiveMetrics}
          label="archive database check commands"
        />
        <Prose>
          <p>
            The archive&apos;s metrics server, like its RPC server, listens on
            every network interface. Block both ports at the firewall, or run
            the archive where only the node can reach them. The card shows these
            values:
          </p>
        </Prose>
        <Terms items={archiveCounts} />
        <Prose>
          <p>The card&apos;s verdict is one of these:</p>
        </Prose>
        <Terms items={archiveVerdicts} />
        <Prose>
          <p>
            When the verdict is Incomplete, <strong>Resend all retained</strong>{" "}
            sends the blocks of the last 870 heights again, and the archive adds
            the ones it lacks. The check has these limits:
          </p>
          <ul>
            <li>
              It shows counts, not which heights are missing. To list them, run{" "}
              <C>mina-missing-blocks-auditor</C> against the database.
            </li>
            <li>
              The archive recomputes the counts every two slots, 3 minutes on
              Mesa. A change in the database shows on the card within about 4
              minutes.
            </li>
            <li>
              Missing heights cover only the archive&apos;s{" "}
              <C>--missing-blocks-width</C>. An older gap doesn&apos;t count.
            </li>
            <li>
              The node sends only its best chain. Blocks of other forks reach
              the database only from other senders, and a height that has any
              block isn&apos;t missing.
            </li>
          </ul>
        </Prose>
        <Prose>
          <h3>Replay a block</h3>
          <p>
            Replay sends a block&apos;s diff to the archive again. The node
            keeps each delivered diff until it is 870 heights below the highest
            block the node accepted. <C>MINA_ARCHIVE_RETAIN_BLOCKS</C> sets that
            window, within the outbox&apos;s size limits. Only blocks in the
            window can be replayed. A replayed diff goes ahead of the diffs that
            wait. The archive keys blocks by state hash, so a replay of a block
            it already has leaves its rows unchanged.
          </p>
          <p>
            <strong>Resend all retained</strong> replays every block in the
            window. The node does the same each time it starts.
          </p>
          <h3>Who may replay</h3>
          <p>
            From a browser, the node takes a replay only from three kinds of
            page: pages of this machine, its own dashboard opened at an IP
            address, and origins you list with <C>--worker-control-origin</C>.
            That option needs <C>--allow-worker-control</C>. A program sends no
            page origin. It sends the request with a header and a JSON body:
          </p>
        </Prose>
        <CodeBlock code={archiveReplay} label="archive replay command" />
        <Prose>
          <p>
            Send <C>{'{"all": true}'}</C> instead to replay every block in the
            window. <C>GET /archive/blocks</C> lists the outbox&apos;s blocks,
            and <C>GET /archive/status</C> reports the feed.
          </p>
          <h3>History outside the window</h3>
          <p>
            A block below the window, or one the node never saw, can&apos;t be
            replayed. The page lists the blocks the node never saw, dropped, or
            found corrupt under <strong>Gaps since the feed started</strong>.
            Import such blocks from precomputed block files with{" "}
            <C>mina-archive-blocks --precomputed</C>.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="on-a-server" title="Open it on a server">
        <Prose>
          <p>
            Don&apos;t open the node&apos;s HTTP port to the internet. Reach the
            dashboard through an SSH tunnel instead. On your own computer, run
            this command:
          </p>
        </Prose>
        <CodeBlock code={sshTunnel} label="SSH tunnel command" />
        <Prose>
          <p>
            Then open <C>http://127.0.0.1:3000</C>. The tunnel stays up until
            you press Ctrl-C. Worker control works through the tunnel, because
            your browser sees the dashboard at a loopback address.
          </p>
          <p>
            If port 3000 is taken on your computer, <C>ssh</C> prints a bind
            error and exits, because of <C>ExitOnForwardFailure=yes</C>. Forward
            another local port instead, as in{" "}
            <C>{sshTunnel.replace("-L 3000", "-L 127.0.0.1:3010")}</C>, and open{" "}
            <C>http://127.0.0.1:3010</C>.
          </p>
          <p>{dashboard.tunnelNote}</p>
          <h3>Over a private network</h3>
          <p>{dashboard.remoteNote}</p>
          <p>
            For example, start the node with <C>--http-addr 192.0.2.103</C> and
            open the dashboard at <C>http://192.0.2.103:3000</C>.
          </p>
          <p>
            To open the dashboard at a hostname instead of an IP address, start
            the node with <C>--http-cors-origin http://NAME:3000</C>.
          </p>
          <p>{dashboard.tailscaleNote}</p>
        </Prose>
        <Caution title="Keep the HTTP port private">
          <p>
            <C>--allow-worker-control</C> lets anyone who can reach the
            node&apos;s HTTP port choose where the fees go. Use it only on a
            node whose HTTP port only you can reach. The node refuses it
            together with <C>--public-graphql</C>.
          </p>
          <p>
            The node takes a change only from three kinds of page: pages of this
            machine, its own dashboard opened at an IP address, and origins you
            list with <C>--worker-control-origin</C>.
          </p>
        </Caution>
      </DocSection>
    </DocsPage>
  );
}
