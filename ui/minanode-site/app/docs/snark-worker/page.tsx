import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";

import { DocsPage } from "@/components/docs/docs-page";
import { C, DocLink, DocSection, Prose } from "@/components/docs/prose";
import { worker } from "@/content/docs";

export const metadata: Metadata = {
  title: "Run a SNARK worker",
  description:
    "Run minanode's SNARK worker on Mina's devnet from the dashboard or with flags: fees, job selection, and the work that blocks buy.",
  alternates: { canonical: "/docs/snark-worker/" },
};

export default function WorkerPage() {
  return (
    <DocsPage
      slug="snark-worker"
      title="Run a SNARK worker"
      lead="A node can prove SNARK work and sell it. For each job, block producers buy the cheapest work in their pool and pay its fee to your public key. The worker needs only that public key, never a private key."
    >
      <Prose>
        <p data-claim={worker.mainnet.claim}>{worker.mainnet.text}</p>
        <p>
          The commands on this page use the release binary. For Docker, follow{" "}
          <DocLink href="/docs/docker/#snark-worker">
            the Docker page&apos;s SNARK worker section
          </DocLink>
          .
        </p>
      </Prose>
      <DocSection id="dashboard" title="Start the worker from the dashboard">
        <CodeBlock
          code="mina node --network devnet --allow-worker-control"
          label="start command with worker control"
        />
        <Prose>
          <p>
            Open the dashboard&apos;s SNARK worker page, enter your public key
            and fee, and start the worker. While the worker runs, you can change
            the key or the fee, or stop it. The node keeps the setting in{" "}
            <C>snark-worker.json</C> in its work directory, so a restart brings
            the worker back.
          </p>
          <p>
            If you don&apos;t have a key yet, make one with{" "}
            <C>mina wallet generate</C>, as for{" "}
            <DocLink href="/docs/block-producer/#key">a block producer</DocLink>
            . The command reads the key&apos;s password from{" "}
            <C>MINA_PRIVKEY_PASS</C>, so set that variable first, as that
            section shows. The worker uses only the public key.
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

      <DocSection id="fee-recipient" title="Choose the fee recipient">
        <Prose>
          <p>
            The fee recipient can be any public key. The worker never needs its
            private key. A recipient with an account on chain can ask any fee. A
            new key has no account yet, so it must ask at least 1 MINA, the
            account creation fee. The web node&apos;s SNARK worker page enters
            the key held or saved in the tab. To ask less with a new key, enter
            any public key that already has an account instead.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="flags" title="Start the worker with flags">
        <CodeBlock
          code={`mina node --network devnet \\
  --run-snarker B62q... --snarker-fee 1`}
          label="start command with a worker"
        />
        <Prose>
          <p>
            <span data-claim={worker.fee.claim}>{worker.fee.text}</span> The
            flags set the worker at every start, and take precedence over a
            setting saved from the dashboard.
          </p>
          <p>
            The example asks 1 MINA because a new key has no account on chain
            yet, and such a key needs at least that. A key that already has an
            account can ask less.
          </p>
          <p>
            If the node runs as the user service, add these flags to its start
            command with a drop-in file.{" "}
            <DocLink href="/docs/block-producer/#service">
              Under the user service
            </DocLink>{" "}
            shows how for the producer&apos;s flags. The worker needs no
            password file.
          </p>
          <p>
            If this node also produces blocks,{" "}
            <C>--snarker-block-first-lead SECONDS</C> stops new worker jobs
            before a known won slot. The default is 420, and the maximum is
            4200. 0 turns the pause off.
          </p>
          <p>
            If the fee receiver is a zkApp account, its receive permission must
            be None. Otherwise OCaml peers reject the work.
          </p>
          <p>
            The <DocLink href="/docs/cli/#node">CLI reference</DocLink> lists
            the SNARK work options.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="fees" title="Compare your fee">
        <Prose>
          <p>
            The dashboard&apos;s SNARK worker page shows what other provers ask,
            and how your fee compares.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="job-selection" title="How the worker chooses a job">
        <Prose>
          <p>
            The worker takes only a job that no other worker has committed to
            and that has no work in the pool as cheap as its fee.{" "}
            <span data-claim={worker.selection.claim}>
              {worker.selection.text}
            </span>{" "}
            <C>--snarker-strategy rand</C> selects this.{" "}
            <C>--snarker-strategy seq</C> takes the first eligible job in
            scan-state order instead.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="bought-work" title="Work that blocks bought">
        <Prose>
          <p>
            The SNARK worker page lists each best-chain block that bought your
            work. For each block, it shows the height, time, producer, the
            bundles and their fees, and the confirmations. The confirmations are
            the blocks on top of it.
          </p>
          <p data-claim={worker.confirmed.claim}>{worker.confirmed.text}</p>
          <p>
            Blocks buy work only when the chain needs proofs. They often buy
            many jobs at once and then none for hours, so a new worker&apos;s
            first sale can be hours away.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="turn-off" title="Turn the worker off">
        <Prose>
          <p>
            Stop the worker in the dashboard, which deletes{" "}
            <C>snark-worker.json</C>. Or stop the node and delete{" "}
            <C>snark-worker.json</C> from the work directory. In both cases,
            start the node without <C>--run-snarker</C>. The record of the work
            that blocks bought, <C>snark-worker-included.json</C>, stays.
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
