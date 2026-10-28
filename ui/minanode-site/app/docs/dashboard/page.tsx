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
import { dashboard } from "@/content/docs";

export const metadata: Metadata = {
  title: "The dashboard",
  description:
    "The dashboard minanode serves on its HTTP port: what each page shows, and how to open it on a server through an SSH tunnel.",
  alternates: { canonical: "/docs/dashboard/" },
};

const pages = [
  {
    label: "Overview",
    value:
      "Sync state, best tip, peers, the saved-state restore and the node's roles",
  },
  {
    label: "SNARK worker",
    value:
      "The worker's state, its start, stop and fee, and the work blocks bought",
  },
  {
    label: "Block producer",
    value:
      "The producer key, won slots ahead, what became of each won slot and its stages, and the totals",
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
    label: "Report a problem",
    value: "A bug report to copy, download or open as a GitHub issue",
  },
];

export default function DashboardPage() {
  return (
    <DocsPage
      slug="dashboard"
      title="The dashboard"
      lead={
        <>
          The node serves its own dashboard on its HTTP port. On the machine it
          runs on, open <C>http://127.0.0.1:3000</C>.
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
            on IPv4 only, and a browser may try <C>localhost</C> as IPv6{" "}
            <C>::1</C> first. Another program on port 3000 there then answers in
            its place, with no error from either.
          </p>
          <p>
            The pages read only this node, and nothing loads from elsewhere.{" "}
            <C>--no-dashboard</C> turns the dashboard off; the API stays on.
          </p>
          <p>
            While the worker is off, the SNARK worker page&apos;s fee field
            starts at the lowest fee on offer in the pool, or 0.1 MINA if there
            is none. A key with no account on chain needs a fee of at least 1
            MINA (
            <DocLink href="/docs/snark-worker/#fee-rule">the rule</DocLink>), so
            raise it for a new key.
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

      <DocSection id="on-a-server" title="Viewing it on a server">
        <Prose>
          <p>
            Don&apos;t open the node&apos;s HTTP port to the internet. Reach the
            dashboard through an SSH tunnel instead: on your own computer, run
          </p>
        </Prose>
        <CodeBlock
          code="ssh -N -L 3000:127.0.0.1:3000 you@server"
          label="SSH tunnel command"
        />
        <Prose>
          <p>
            and open <C>http://127.0.0.1:3000</C>. The tunnel stays up until you
            press Ctrl-C. Worker control works through it, because your browser
            sees the dashboard at a loopback address.
          </p>
          <p>
            If port 3000 is taken on your computer, forward another local port,
            as in <C>ssh -N -L 127.0.0.1:3010:127.0.0.1:3000 you@server</C>, and
            open <C>http://127.0.0.1:3010</C>. When the local port is taken,{" "}
            <C>ssh</C> prints a bind error but keeps running, so check its
            output if the page fails to load.
          </p>
          <p>{dashboard.tunnelNote}</p>
          <p>{dashboard.remoteNote}</p>
          <p>
            For example, use <C>--http-addr 192.0.2.103</C> and open the
            dashboard at <C>http://192.0.2.103:3000</C>.
          </p>
          <p>
            To open it at a hostname instead of an IP address, start the node
            with <C>--http-cors-origin http://NAME:3000</C>.
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
            A change is taken only from pages of this machine, from the
            node&apos;s own dashboard opened at an IP address, and from origins
            you list with <C>--worker-control-origin</C>.
          </p>
        </Caution>
      </DocSection>
    </DocsPage>
  );
}
