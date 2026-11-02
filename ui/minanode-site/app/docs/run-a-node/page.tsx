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
      lead="Start the node as a devnet full node, keep it running, and fix common problems. The sections after Troubleshooting describe the node's files, ports, and APIs."
    >
      <DocSection id="start" title="Start the node">
        <CodeBlock code="mina node --network devnet" label="start command" />
        <Prose>
          <p>
            The node finds peers, syncs, and then follows the tip of the chain.
            Watch it in the dashboard at <C>http://127.0.0.1:3000</C>. To stop
            it, press Ctrl-C.
          </p>
          <p>
            The node runs in the foreground, so it stops when you close the
            terminal or log out. To keep it running and start it again after a
            reboot, <DocLink href="#service">run it in the background</DocLink>{" "}
            as a service.
          </p>
          <p>
            On a server, open the dashboard through an{" "}
            <DocLink href="/docs/dashboard/#on-a-server">SSH tunnel</DocLink>.
            From another shell, <C>mina node status</C> reports the node&apos;s
            state.
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

      <DocSection id="service" title="Run in the background">
        <Prose>
          <p>
            With systemd, a user service keeps the node running after you log
            out, and starts it again after a reboot. The service runs the binary
            from <DocLink href="/docs/install/#run">Install</DocLink>. It uses
            the same work directory, <C>~/.mina</C>, and the same dashboard
            port, 3000, as the foreground command, so it keeps any chain you
            have already synced.
          </p>
          <ol>
            <li>If a node runs in a terminal, stop it with Ctrl-C.</li>
            <li>
              Create the unit&apos;s directory with{" "}
              <C>mkdir -p ~/.config/systemd/user</C>.
            </li>
            <li>
              Save this unit as <C>~/.config/systemd/user/minanode.service</C>.
              The name keeps it apart from the OCaml package&apos;s{" "}
              <C>mina.service</C>.
            </li>
          </ol>
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
            With <C>--disable-filesystem-logging</C>, the logs go to the journal
            instead of <C>mina.log</C>. Everything else is the foreground
            default. Start the service and follow its log:
          </p>
        </Prose>
        <CodeBlock
          code={`systemctl --user daemon-reload
systemctl --user enable --now minanode.service
loginctl enable-linger "$USER"
journalctl --user -u minanode.service -f`}
          label="start the service and follow its log"
        />
        <Prose>
          <p>
            Linger keeps your user service manager running after logout, and
            starts it at boot. If <C>loginctl</C> refuses to enable linger, ask
            the server administrator to enable it for your account.
          </p>
          <p>
            The dashboard stays at <C>http://127.0.0.1:3000</C>. On a server,
            use the{" "}
            <DocLink href="/docs/dashboard/#on-a-server">SSH tunnel</DocLink>.
            To stop the service, run{" "}
            <C>systemctl --user stop minanode.service</C>. For a bug report,
            take the log from the journal:
          </p>
        </Prose>
        <CodeBlock
          code={`mina node status
journalctl --user -u minanode.service | mina report --log -`}
          label="service status and report"
        />
      </DocSection>

      <DocSection id="troubleshooting" title="Troubleshooting">
        <Prose>
          <h3>Sync seems stuck</h3>
          <p>The sync state names the phase the node is in:</p>
          <ul>
            <li>Starting: the node has not reported a phase yet.</li>
            <li>Connecting: no peer has connected.</li>
            <li>Listening: peers have connected but have not sent a tip.</li>
            <li>Bootstrap: the node fetches ledgers and blocks.</li>
            <li>Catchup: the node applies blocks to reach the tip.</li>
            <li>Synced: the node has caught up.</li>
          </ul>
          <p>
            Before the first Synced, warnings such as <C>Ledger unavailable</C>{" "}
            or <C>TransactionPoolFetchAccountsFailed</C> are normal. The
            transaction pool asks for accounts before the ledger is ready. If
            the warnings continue after Synced, collect a report.
          </p>
          <p>
            Some other log lines are harmless. A first start logs{" "}
            <C>no ledger to copy for sync, starting from an empty one</C> and a
            stray <C>[-] no FUZZER conf specified</C> line. Kad warnings such as{" "}
            <C>Skipping updates to Kad entry multiaddress list</C> can repeat at
            any time. A producer key with no stake logs{" "}
            <C>Empty delegator table</C>.
          </p>
          <p>
            Before you restart the node, check the peer count, the target
            height, and the progress in the log. Ledger downloads take time, and
            a quiet network can leave the tip unchanged. If peers advance but
            your node makes no progress, or it repeats errors, collect a report
            before you delete any data. If you changed the work directory or the
            HTTP port, change them in these commands too.
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
            <DocLink href="/docs/bug-reports/">Report a bug</DocLink>.
          </p>
          <h3>The dashboard does not open on a server</h3>
          <p>
            The HTTP port listens on 127.0.0.1 by default. Use an{" "}
            <DocLink href="/docs/dashboard/#on-a-server">SSH tunnel</DocLink> to
            the node&apos;s HTTP port.
          </p>
          <h3>Port already in use</h3>
          <p>
            If the HTTP listener or the WebRTC signaling listener cannot bind,
            the node stops at startup. The error names the flag and the port. An
            enabled <C>--webrtc-signal-port</C> needs a port of its own when its
            listening address overlaps <C>--http-addr</C>. A wildcard address
            such as <C>0.0.0.0</C> overlaps <C>127.0.0.1</C> too.
          </p>
          <p>
            Choose an unused HTTP port with <C>--port</C> and an unused peer
            port with <C>--libp2p-port</C>. Then change the dashboard URL, the
            tunnel, and the status command to match. For a second node, also use
            a separate work directory. Do not stop another service to free its
            port.
          </p>
        </Prose>
        <CodeBlock
          code={`mina node --network devnet -d ~/.mina-second \\
  --port 3010 --libp2p-port 8312`}
          label="node with separate ports and data"
        />
        <Prose>
          <h3>Command not found, or another mina</h3>
          <p>
            Run the installed binary by its full path. Then add its directory to
            PATH in this shell, and in your shell startup file for later shells.
          </p>
          <p>
            If <C>mina --version</C> prints anything but this release&apos;s{" "}
            <C>mina</C> line, another program called <C>mina</C> comes first on
            PATH. The OCaml node is one example. <C>command -v mina</C> shows
            which one runs. Put <C>~/.local/bin</C> first, as below, or use the
            full path.
          </p>
        </Prose>
        <CodeBlock
          code={`~/.local/bin/mina --version
export PATH="$HOME/.local/bin:$PATH"
command -v mina
mina --version`}
          label="check the install and set PATH"
        />
      </DocSection>

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
            The node builds the verifier index cache at the first start, in{" "}
            <C>~/.mina/circuit-blobs/</C>, whatever the work directory.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="ports" title="Ports">
        <KeyValueList title="Default ports" rows={claimedRows(run.ports)} />
        <Prose>
          <p>
            By default, the HTTP port listens on <C>127.0.0.1</C> only.{" "}
            <C>--http-addr</C>, or <C>MINA_HTTP_ADDR</C>, sets another listening
            address, such as a private interface. <C>--http-addr 0.0.0.0</C>{" "}
            listens on all IPv4 interfaces. Before you change it, read{" "}
            <DocLink href="/docs/dashboard/#on-a-server">
              Open it on a server
            </DocLink>
            . The peer port is separate, and other nodes can reach it.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="restarts" title="Saved state and restarts">
        <Prose>
          <p>
            The node saves the chain state as it runs. A restart picks up from
            the saved state instead of syncing from the start.
          </p>
          <p>
            The first save comes a few seconds after the node first syncs, then
            one every 30 minutes. The log says <C>checkpoint written</C> each
            time. A node stopped before its first save syncs from the start
            again.
          </p>
          <p>
            At a restart, the node checks the saved state first. If the state is
            too old or fails a check, the node syncs afresh and says why in its
            log. If restoring the state crashes the node three times, the node
            deletes it. A stop during a restore does not count as a crash,
            whether by Ctrl-C, <C>systemctl stop</C>, or <C>docker stop</C>.
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
            the work directory. Disk use varies with the pool. The node frees a
            file when it drops the last reference to it, and a restart clears
            old cache files. Running nodes must not share a work directory.
          </p>
          <p>
            <C>--no-proof-cache</C> keeps those proofs in memory instead. A
            block producer keeps them in memory unless it starts with{" "}
            <C>--proof-cache</C>. This request shows the cache&apos;s files,
            bytes, and errors:
          </p>
        </Prose>
        <CodeBlock
          code="curl -s http://127.0.0.1:3000/stats/memory/proofs"
          label="proof cache statistics"
        />
      </DocSection>

      <DocSection id="logs" title="Logs">
        <Prose>
          <p data-claim={run.logs.claim}>{run.logs.text}</p>
          <p>
            <C>--log-level debug</C> logs more detail. The user service in{" "}
            <DocLink href="#service">Run in the background</DocLink> logs to the
            journal.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="graphql" title="GraphQL">
        <Prose>
          <p>
            The node answers GraphQL queries at <C>/graphql</C> on its HTTP
            port, for wallets, explorers, and scripts. The dashboard&apos;s
            GraphQL page shows the address and an example to copy.
          </p>
        </Prose>
        <CodeBlock code={query} label="GraphQL query with curl" />
        <Prose>
          <p>
            The mutations that send transactions, <C>sendPayment</C>,{" "}
            <C>sendDelegation</C>, and <C>sendZkapp</C>, are off unless the node
            starts with <C>--enable-tx-submission</C>. Turn them on only for a
            node whose HTTP port only you can reach.
          </p>
          <p>
            A request can carry up to 1.25 MiB of body and of query text, and up
            to 24,576 tokens. These limits admit the largest zkApp command that
            Mesa accepts, with 15 proofs, as o1js builds it inline in the
            mutation. A valid command padded with many empty events can still
            exceed the limits. The node admits bodies over 256 KiB one at a
            time. Another large body waits up to 30 s, then gets HTTP 503.
          </p>
          <p>
            <C>--public-graphql</C> keeps the earlier limits and refuses
            mutations. The earlier limits are 256 KiB of body, 64 KiB of query
            text, and 8,192 tokens.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="http-api" title="HTTP API">
        <Prose>
          <p>
            <C>GET /status</C> answers with the node&apos;s sync state, best
            tip, and peers as JSON. <C>GET /build_env</C> answers with its
            build. The dashboard&apos;s Status page shows both, and{" "}
            <C>mina node status</C> sums them up.
          </p>
          <p>
            Light clients read the best tip and account Merkle paths from{" "}
            <C>/light/*</C>, as{" "}
            <DocLink href="/docs/light-endpoints/">
              Light client endpoints
            </DocLink>{" "}
            describes.
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
