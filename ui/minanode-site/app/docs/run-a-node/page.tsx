import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";

import { DocsPage } from "@/components/docs/docs-page";
import {
  C,
  DocLink,
  DocSection,
  Prose,
  Terms,
  claimedRows,
} from "@/components/docs/prose";
import { run } from "@/content/docs";

export const metadata: Metadata = {
  title: "Run a node",
  description:
    "Start minanode on Mina's devnet: the work directory, ports, saved state and restarts, logs, and the node's API.",
  alternates: { canonical: "/docs/run-a-node/" },
};

const query = `curl -s http://127.0.0.1:3000/graphql \\
  -H 'content-type: application/json' \\
  -d '{"query": "{ syncStatus bestChain(maxLength: 1) { stateHash } }"}'`;

export default function RunPage() {
  return (
    <DocsPage
      slug="run-a-node"
      title="Run a node"
      lead="Start the node, and it joins devnet as a full node."
    >
      <DocSection id="start" title="Start it">
        <CodeBlock code="mina node --network devnet" label="start command" />
        <Prose>
          <p>
            The node finds peers, syncs, then follows the tip of the chain.
            Watch it in the dashboard at <C>http://127.0.0.1:3000</C>. Stop it
            with Ctrl-C.
          </p>
          <p>
            The node runs in the foreground: it stops when you close the
            terminal or log out. To keep it running, and start it again after a
            reboot, <DocLink href="#service">run it in the background</DocLink>{" "}
            as a service.
          </p>
          <p>
            On a server, open the dashboard through an{" "}
            <DocLink href="/docs/dashboard/#on-a-server">SSH tunnel</DocLink>.{" "}
            <C>mina node status</C> reports the node&apos;s state from another
            shell.
          </p>
          <p>
            The <DocLink href="/docs/cli/#node">CLI reference</DocLink> lists
            every option of <C>mina node</C>.
          </p>
        </Prose>
        <CodeBlock code="mina node status" label="status from another shell" />
      </DocSection>

      {/* TODO(SYNC-DEMO): Keep this draft off the rendered site until RC6 is
          measured and reviewed. Add its evidence to CLAIMS.md before enabling. */}
      {false && (
        <DocSection id="sync-time" title="How fast does it sync?">
          <Prose>
            <p>
              RC6 syncs a full devnet node from an empty directory in
              [RC6_MEDIAN_SECONDS] seconds at the median of five runs.
            </p>
            <p>The p90 is [RC6_P90_SECONDS] seconds.</p>
            <p>Peak RSS is [RC6_PEAK_RSS_MIB] MiB at the median.</p>
            <p>
              These runs used [RC6_CPU], [RC6_MEMORY], [RC6_STORAGE] and
              [RC6_CONNECTION].
            </p>
            <p>
              Sync includes the staking, next epoch and root ledgers plus the
              recent chain of about 290 blocks.
            </p>
            <p>
              Install the verified release from the install page. From a
              checkout of that release, run the commands below with Bash and
              Python 3. Each run starts with an empty data directory and empty
              caches. The operating system page cache is not cleared.
            </p>
          </Prose>
          <CodeBlock
            code={`tools/demo/fresh-sync.sh --network devnet --binary ~/.local/bin/mina --keep
python3 tools/demo/sync-matrix.py --network devnet --runs 5 \\
  --binary ~/.local/bin/mina --output sync-rc6`}
            label="reproduce the sync measurement"
          />
        </DocSection>
      )}

      <DocSection id="work-directory" title="The work directory">
        <Prose>
          <p data-claim={run.workDir.claim}>
            The node keeps its data in <C>~/.mina</C>. Use <C>-d DIR</C>, or set{" "}
            <C>MINA_HOME</C>, for another directory.
          </p>
        </Prose>
        <Terms
          mono
          items={[
            { term: "<chain id>/frontier/", text: "The saved chain state" },
            {
              term: "ledger-accounts/",
              text: "The accounts of the staking, next epoch and root ledgers",
            },
            { term: "mina.log, mina.log.1, ...", text: "The logs" },
            {
              term: "p2p-secret-key",
              text: "The node's peer identity, made at the first start",
            },
            {
              term: "proof-cache/",
              text: "The SNARK pool's cached proofs",
            },
            {
              term: "snark-worker.json",
              text: "The worker setting chosen in the dashboard",
            },
            {
              term: "snark-worker-included.json",
              text: "The worker's work that blocks included",
            },
            {
              term: "vrf-evaluation/",
              text: "A block producer's saved slot checks",
            },
            {
              term: "block-production.json",
              text: "A block producer's won slots and what became of them",
            },
            { term: "crashes/", text: "A report of each crash" },
            {
              term: "last-start.json",
              text: "How the node was started, for bug reports",
            },
            { term: "bug-reports/", text: "Reports mina report saved" },
          ]}
        />
        <Prose>
          <p>
            The verifier index cache, built at the first start, is in{" "}
            <C>~/.mina/circuit-blobs/</C> whatever the work directory.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="ports" title="Ports">
        <KeyValueList title="Default ports" rows={claimedRows(run.ports)} />
        <Prose>
          <p>
            The HTTP port listens on <C>127.0.0.1</C> only by default.{" "}
            <C>--http-addr</C> (or <C>MINA_HTTP_ADDR</C>) sets another listening
            address, such as a private interface, and <C>--http-addr 0.0.0.0</C>{" "}
            listens on all IPv4 interfaces. Before changing it, read{" "}
            <DocLink href="/docs/dashboard/#on-a-server">
              viewing it on a server
            </DocLink>
            . The peer port is separate and may be open to other nodes.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="restarts" title="Saved state and restarts">
        <Prose>
          <p>
            The node saves the chain state as it goes, and a restart picks up
            from it instead of syncing from the start. It checks the saved state
            first: if the state is too old or fails a check, the node syncs
            afresh and says why in its log. If restoring it crashes the node
            three times, the node deletes it. A stop during a restore (Ctrl-C,{" "}
            <C>systemctl stop</C>, <C>docker stop</C>) doesn&apos;t count.
          </p>
          <p>
            <C>--no-frontier-store</C> turns saving off.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="proof-cache" title="Proof cache">
        <Prose>
          <p>
            The SNARK pool caches proofs on disk in <C>proof-cache/</C> under
            the work directory. Disk use varies with the pool. Files are freed
            when their last reference is dropped; a restart clears old cache
            files. Do not share a work directory between running nodes.
          </p>
          <p>
            <C>--no-proof-cache</C> keeps those proofs in memory instead. A
            block producer keeps them in memory unless started with{" "}
            <C>--proof-cache</C>. To inspect cache files, bytes and errors:
          </p>
        </Prose>
        <CodeBlock
          code="curl -s http://127.0.0.1:3000/stats/memory/proofs"
          label="proof cache statistics"
        />
      </DocSection>

      <DocSection id="service" title="Run in the background">
        <Prose>
          <p>
            With systemd, a user service keeps the node running after you log
            out and starts it again after a reboot. It runs the binary from{" "}
            <DocLink href="/docs/install/#run">the install page</DocLink> with
            the same work directory (<C>~/.mina</C>) and dashboard port (3000)
            as the foreground command, so it keeps any chain you have already
            synced. Stop a node running in a terminal first, with Ctrl-C.
          </p>
          <p>
            Save this as <C>~/.config/systemd/user/mina.service</C>:
          </p>
        </Prose>
        <CodeBlock
          code={`[Unit]
Description=Mina devnet node

[Service]
ExecStart=%h/.local/bin/mina node --network devnet \\
  --disable-filesystem-logging
Restart=on-failure

[Install]
WantedBy=default.target`}
          label="systemd user unit"
        />
        <Prose>
          <p>
            <C>--disable-filesystem-logging</C> leaves the logs to the journal
            instead of <C>mina.log</C>. Everything else is the foreground
            default.
          </p>
        </Prose>
        <CodeBlock
          code={`mkdir -p ~/.config/systemd/user
# Save the unit above, then:
systemctl --user daemon-reload
systemctl --user enable --now mina.service
loginctl enable-linger "$USER"
journalctl --user -u mina.service -f`}
          label="start the service and follow its log"
        />
        <Prose>
          <p>
            Linger keeps your user service manager running after logout and
            starts it at boot. If enabling linger is refused, ask the server
            administrator to enable it for your account.
          </p>
          <p>
            The dashboard stays at <C>http://127.0.0.1:3000</C>; on a server,
            use the{" "}
            <DocLink href="/docs/dashboard/#on-a-server">SSH tunnel</DocLink>.
            Stop the service with <C>systemctl --user stop mina.service</C>. For
            a bug report, take the log from the journal:
          </p>
        </Prose>
        <CodeBlock
          code={`mina node status
journalctl --user -u mina.service | mina report --log -`}
          label="service status and report"
        />
      </DocSection>

      <DocSection id="troubleshooting" title="Troubleshooting">
        <Prose>
          <h3>Sync seems stuck</h3>
          <p>
            Connecting means no peer has connected. Listening means peers have
            connected but have not sent a tip. Bootstrap fetches ledgers and
            blocks. Catchup applies blocks to reach the tip. Synced means the
            node has caught up. Starting means it has not reported a phase yet.
          </p>
          <p>
            Check the peer count, target height and log progress before
            restarting. Ledger downloads take time, and a quiet network may
            leave the tip unchanged. If peers advance but your node makes no
            progress, or it repeats errors, collect a report before deleting
            data. Replace the directory and URL below if you changed them.
          </p>
        </Prose>
        <CodeBlock
          code={`mina node status --node http://127.0.0.1:3000
mina report -d ~/.mina --node http://127.0.0.1:3000 \\
  -m "Sync stopped progressing" --output report.md`}
          label="check sync and save a report"
        />
        <Prose>
          <p>
            Review the report, then follow{" "}
            <DocLink href="/docs/bug-reports/">reporting a bug</DocLink>.
          </p>
          <h3>The dashboard will not open on a server</h3>
          <p>
            HTTP listens on 127.0.0.1 by default. Use an{" "}
            <DocLink href="/docs/dashboard/#on-a-server">SSH tunnel</DocLink> to
            the node&apos;s HTTP port.
          </p>
          <h3>Port already in use</h3>
          <p>
            The node stops startup if its operator HTTP or signaling listener
            cannot bind, and the error names the flag and port. An enabled{" "}
            <C>--webrtc-signal-port</C> must use a different port when its
            listening address overlaps <C>--http-addr</C>. A wildcard address
            such as <C>0.0.0.0</C> also overlaps <C>127.0.0.1</C>.
          </p>
          <p>
            Choose an unused HTTP port with <C>--port</C> and an unused peer
            port with <C>--libp2p-port</C>. Update the dashboard URL, tunnel and
            status command to match. For a second node, also use a separate work
            directory. Do not stop another service to free its port.
          </p>
        </Prose>
        <CodeBlock
          code={`mina node --network devnet -d ~/.mina-second \\
  --port 3010 --libp2p-port 8312`}
          label="node with separate ports and data"
        />
        <Prose>
          <h3>Command not found</h3>
          <p>
            Try the installed binary by its full path. Add its directory to PATH
            in this shell, and in your shell startup file for later shells.
          </p>
        </Prose>
        <CodeBlock
          code={`~/.local/bin/mina --version
export PATH="$HOME/.local/bin:$PATH"
mina --version`}
          label="check the install and set PATH"
        />
      </DocSection>

      <DocSection id="logs" title="Logs">
        <Prose>
          <p data-claim={run.logs.claim}>{run.logs.text}</p>
          <p>
            <C>--log-level debug</C> logs more detail. The user service logs to
            the journal (see{" "}
            <DocLink href="#service">run in the background</DocLink>).
          </p>
        </Prose>
      </DocSection>

      <DocSection id="graphql" title="GraphQL">
        <Prose>
          <p>
            The node answers GraphQL queries at <C>/graphql</C> on its HTTP
            port, for wallets, explorers and scripts. The dashboard&apos;s
            GraphQL page shows the address and an example to copy.
          </p>
        </Prose>
        <CodeBlock code={query} label="GraphQL query with curl" />
        <Prose>
          <p>
            Sending transactions (<C>sendPayment</C>, <C>sendDelegation</C>,{" "}
            <C>sendZkapp</C>) is off unless you start the node with{" "}
            <C>--enable-tx-submission</C>. Turn it on only for a node whose HTTP
            port only you can reach.
          </p>
          <p>
            A request may carry up to 1.25 MiB of body and of query text, and up
            to 24,576 tokens. That admits the largest zkApp command Mesa accepts
            (15 proofs) as o1js builds it, inline in the mutation. A valid
            command padded with many empty events can still exceed the limits.
            The node admits bodies over 256 KiB one at a time. Another large
            body waits up to 30 s, then gets HTTP 503.
          </p>
          <p>
            <C>--public-graphql</C> keeps the earlier limits (256 KiB of body,
            64 KiB of query text and 8,192 tokens) and refuses mutations.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="http-api" title="HTTP API">
        <Prose>
          <p>
            <C>GET /status</C> answers with the node&apos;s sync state, best tip
            and peers as JSON, and <C>GET /build_env</C> with its build. The
            dashboard&apos;s Status page shows both, and <C>mina node status</C>{" "}
            sums them up.
          </p>
          <p>
            Light clients read the best tip and account Merkle paths from{" "}
            <C>/light/*</C>: see{" "}
            <DocLink href="/docs/light-endpoints/">
              light client endpoints
            </DocLink>
            .
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
