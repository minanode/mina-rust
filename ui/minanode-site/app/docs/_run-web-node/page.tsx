import type { Metadata } from "next";
import { CodeBlock } from "@minanode/design-system/patterns/code-block";
import { DocsPage } from "@/components/docs/docs-page";
import { C, DocLink, DocSection, Prose } from "@/components/docs/prose";
import { release } from "@/content/release";

export const metadata: Metadata = {
  title: "Run the web node yourself",
  description:
    "Download, verify and run the Mina devnet web node on your own computer.",
  alternates: { canonical: "/docs/run-web-node/" },
};

// Private route until the owner publishes the web archive and signed sums.
export default function RunWebNodePage() {
  const archive = `mina-web-${release.version}.tar.gz`;
  return (
    <DocsPage
      slug="run-web-node"
      title="Run the web node yourself"
      lead="Run the web node from a release you have checked, with its JavaScript and WASM served from your own computer."
    >
      <DocSection id="download" title="Download and check">
        <Prose>
          <p>
            This is an experimental devnet node. Use only devnet keys. You need
            Python 3.11 or newer and a current desktop Chrome or Chromium
            browser.
          </p>
          <p>
            Download{" "}
            <DocLink href={`${release.downloadUrl}/${archive}`}>
              {archive}
            </DocLink>
            , then follow{" "}
            <DocLink href="/docs/install/#check">
              the release signature and checksum checks
            </DocLink>{" "}
            before unpacking it. Get <C>SHA256SUMS</C> and <C>SHA256SUMS.sig</C>{" "}
            from the same release.
          </p>
          <p>
            The archive includes its prover files, genesis configuration, fonts
            and verifier caches. It needs no external script or circuit
            download. You still trust the release publisher, your browser and
            your computer.
          </p>
        </Prose>
        <CodeBlock
          label="unpack and run"
          code={`tar -xzf ${archive}\ncd mina-web-${release.version}\npython3 -B serve.py`}
        />
      </DocSection>
      <DocSection id="start" title="Open the node">
        <Prose>
          <p>
            Open <DocLink href="http://127.0.0.1:8080/">127.0.0.1:8080</DocLink>
            . The server checks the bundle manifest before listening and sends
            the isolation headers needed by WASM threads. Opening the HTML file
            directly will not work.
          </p>
          <p>
            Use <C>--port 8081</C> if the port is busy. Press Ctrl-C to stop the
            server. To check the unpacked files without starting it, run{" "}
            <C>python3 -B serve.py --verify-only</C>.
          </p>
        </Prose>
      </DocSection>
      <DocSection id="relay" title="Choose a relay">
        <Prose>
          <p>
            The default is the release relay recorded in <C>manifest.json</C>.
            The browser dials exactly that relay, with discovery disabled. The
            relay and STUN servers still see your network address; a restricted
            network may block WebRTC.
          </p>
          <p>
            To use your own native devnet node, start it in another terminal
            from the bundle directory. Keep its data in the sibling directory
            below so the bundle stays unchanged:
          </p>
        </Prose>
        <CodeBlock
          label="start your local relay"
          code={`mina node --network devnet --work-dir ../mina-relay-data \\\n  --http-addr 127.0.0.1 --port 3000 --libp2p-port 8302 \\\n  --webrtc-signal-addr 127.0.0.1 --webrtc-signal-port 8303`}
        />
        <Prose>
          <p>
            Wait for it to sync. Read the public peer ID from its{" "}
            <C>P2P identity</C> log line and use it in place of <C>PEER_ID</C>:
          </p>
        </Prose>
        <CodeBlock
          label="use your relay"
          code={`python3 -B serve.py \\\n  --relay /ip4/127.0.0.1/tcp/8303/webrtc/http/p2p/PEER_ID`}
        />
        <Prose>
          <p>
            The option replaces the packaged relay without rebuilding or
            changing the signed files. Restart the server and reload the page
            when changing it. Leave <C>--webrtc-signaling-relay</C> disabled.
            For a remote relay, use its reachable signaling address and
            configure its WebRTC UDP ports and NAT as described in the archive
            README.
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
