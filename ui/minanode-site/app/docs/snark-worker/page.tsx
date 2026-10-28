import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";

import { DocsPage } from "@/components/docs/docs-page";
import { C, DocLink, DocSection, Prose } from "@/components/docs/prose";
import { worker } from "@/content/docs";

export const metadata: Metadata = {
  title: "SNARK worker",
  description:
    "Run minanode's SNARK worker on Mina's devnet from the dashboard or with flags: fees, job selection and the work blocks buy.",
  alternates: { canonical: "/docs/snark-worker/" },
};

export default function WorkerPage() {
  return (
    <DocsPage
      slug="snark-worker"
      title="SNARK worker"
      lead="A node may prove SNARK work and sell it. For each job, block producers buy the cheapest work in their pool and pay its fee to your public key. The worker needs only that public key, never a private key."
    >
      <Prose>
        <p>
          The commands on this page use the release binary. In Docker, see{" "}
          <DocLink href="/docs/docker/#snark-worker">
            the Docker page&apos;s SNARK worker section
          </DocLink>
          .
        </p>
      </Prose>
      <DocSection id="dashboard" title="Start it from the dashboard">
        <CodeBlock
          code="mina node --network devnet --allow-worker-control"
          label="start command with worker control"
        />
        <Prose>
          <p>
            Open the dashboard&apos;s SNARK worker page, enter your public key
            and fee, and start the worker. While it runs, you may change the key
            or the fee, or stop it. The node keeps the setting in{" "}
            <C>snark-worker.json</C> in its work directory, so a restart brings
            the worker back.
          </p>
          <p>
            Without a key yet, make one with <C>mina wallet generate</C>, as for{" "}
            <DocLink href="/docs/block-producer/#key">a block producer</DocLink>
            . Set <C>MINA_PRIVKEY_PASS</C> first, as that section shows: the
            command reads the key&apos;s password from it. The worker uses only
            the public key.
          </p>
          <p
            id="fee-rule"
            className="scroll-mt-24"
            data-claim={worker.feeRule.claim}
          >
            {worker.feeRule.text}
          </p>
          <p>
            <C>--allow-worker-control</C> lets anyone who can reach the
            node&apos;s HTTP port choose where the fees go. On a server, use an{" "}
            <DocLink href="/docs/dashboard/#on-a-server">SSH tunnel</DocLink>.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="flags" title="Start it with flags">
        <CodeBlock
          code={`mina node --network devnet \\
  --run-snarker B62q... --snarker-fee 1`}
          label="start command with a worker"
        />
        <Prose>
          <p>
            <span data-claim={worker.fee.claim}>{worker.fee.text}</span> The
            flags set the worker at every start, and win over a setting saved
            from the dashboard.
          </p>
          <p>
            The example asks 1 MINA because a new key has no account on chain
            yet, and such a key needs at least that. A key that already has an
            account may ask less.
          </p>
          <p>
            If the node runs as the user service, add these flags to its start
            command with a drop-in file, as the{" "}
            <DocLink href="/docs/block-producer/#service">
              block producer page
            </DocLink>{" "}
            shows for its flags. The worker needs no password file.
          </p>
          <p>
            If this node also produces blocks,{" "}
            <C>--snarker-block-first-lead SECONDS</C> stops new worker jobs
            before a known won slot (default 420, maximum 4200; 0 disables the
            pause).
          </p>
          <p>
            If the fee receiver is a zkApp account, its receive permission must
            be None or OCaml peers reject the work.
          </p>
          <p>
            The <DocLink href="/docs/cli/#node">CLI reference</DocLink> lists
            the SNARK work options.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="fees" title="Fees">
        <Prose>
          <p>
            The SNARK worker page shows what other provers ask and how your fee
            compares.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="job-selection" title="Job selection">
        <Prose>
          <p>
            The worker takes only a job that no other worker has committed to
            and that has no work in the pool as cheap as its fee.{" "}
            <span data-claim={worker.selection.claim}>
              {worker.selection.text}
            </span>{" "}
            <C>--snarker-strategy rand</C> selects this; <C>seq</C> takes the
            first eligible job in scan-state order.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="bought-work" title="Work bought by blocks">
        <Prose>
          <p>
            The SNARK worker page lists each best-chain block that bought your
            work: its height, time, producer, the bundles and their fees, and
            its confirmations (the blocks on top of it).
          </p>
          <p data-claim={worker.confirmed.claim}>{worker.confirmed.text}</p>
          <p>
            Blocks buy work only when the chain needs proofs, often many jobs at
            once and then none for hours, so a new worker&apos;s first sale may
            be hours away.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="turn-off" title="Turn it off">
        <Prose>
          <p>
            Stop the worker in the dashboard, which deletes{" "}
            <C>snark-worker.json</C>, or stop the node and delete{" "}
            <C>snark-worker.json</C> from the work directory. In both cases,
            start the node without <C>--run-snarker</C>. The record of work
            blocks bought, <C>snark-worker-included.json</C>, stays.
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
