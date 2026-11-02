import type { Metadata } from "next";
import { CodeBlock } from "@minanode/design-system/patterns/code-block";
import { DocsPage } from "@/components/docs/docs-page";
import { C, DocLink, DocSection, Prose } from "@/components/docs/prose";
import { webRelease } from "@/content/release";

export const metadata: Metadata = {
  title: "Run the web node yourself",
  description:
    "Download, verify, and run the Mina devnet web node on your own computer.",
  alternates: { canonical: "/docs/run-web-node/" },
};

export default function RunWebNodePage() {
  const archive = `mina-web-${webRelease.version}.tar.gz`;
  return (
    <DocsPage
      slug="run-web-node"
      title="Run the web node yourself"
      lead="Run the web node from a release you have checked, with its JavaScript and WASM served from your own computer."
    >
      <DocSection id="download" title="Download and check the archive">
        <Prose>
          <p>
            The web node also runs at{" "}
            <DocLink href="https://minaweb.org">minaweb.org</DocLink>. This page
            runs the same devnet node from a release archive instead.
          </p>
          <p>
            The web node is an experimental devnet node. Use only devnet keys.
            You need Python 3.11 or newer, and a current desktop Chrome or
            Chromium browser.
          </p>
          <ol>
            <li>
              Download{" "}
              <DocLink href={`${webRelease.downloadUrl}/${archive}`}>
                {archive}
              </DocLink>
              , with <C>SHA256SUMS</C> and <C>SHA256SUMS.sig</C> from the same
              release.
            </li>
            <li>
              Before you unpack the archive, verify it as{" "}
              <DocLink href="/docs/install/#check">
                Verify the signature and the checksum
              </DocLink>{" "}
              describes.
            </li>
            <li>Unpack the archive and start its server:</li>
          </ol>
        </Prose>
        <CodeBlock
          label="unpack and run"
          code={`tar -xzf ${archive}\ncd mina-web-${webRelease.version}\npython3 -B serve.py`}
        />
        <Prose>
          <p>
            The archive holds the page, its WASM, the prover files, and the
            genesis configuration. The node builds its verifier indexes when it
            starts, and needs no other code or circuit download. You still trust
            the release publisher, your browser, and your computer.
          </p>
        </Prose>
      </DocSection>
      <DocSection id="start" title="Open the node">
        <Prose>
          <p>
            Open <DocLink href="http://127.0.0.1:8080/">127.0.0.1:8080</DocLink>{" "}
            and press <strong>Start</strong>. The page does not work if you open
            the HTML file directly.
          </p>
          <p>
            Before it listens, the server checks every file against{" "}
            <C>manifest.json</C>. It listens only on <C>127.0.0.1</C>. It sends
            the isolation headers that WASM threads need, and a
            Content-Security-Policy that lets the page fetch only from itself
            and its relays.
          </p>
          <p>
            If port 8080 is busy, add <C>--port 8081</C>. To stop the server,
            press Ctrl-C. To check the unpacked files without starting the
            server, run <C>python3 -B serve.py --verify-only</C>.
          </p>
        </Prose>
      </DocSection>
      <DocSection id="relay" title="Choose the relays">
        <Prose>
          <p>
            By default, the node dials the release&apos;s relays, which{" "}
            <C>manifest.json</C> lists. It dials only those, with discovery off.
            The relays see your network address, and so do the public STUN
            servers that the node uses, Google&apos;s among them. The archive
            has no TURN servers, so a network that blocks direct WebRTC blocks
            the node.
          </p>
          <p>
            To use your own native devnet node as the relay, start it in another
            terminal from the bundle directory. Keep its data in the sibling
            directory below, so the bundle stays unchanged:
          </p>
        </Prose>
        <CodeBlock
          label="start your local relay"
          code={`mina node --network devnet --work-dir ../mina-relay-data \\\n  --http-addr 127.0.0.1 --port 3000 --libp2p-port 8302 \\\n  --webrtc-signal-addr 127.0.0.1 --webrtc-signal-port 8303`}
        />
        <Prose>
          <p>
            Wait for the node to sync. Then read its public peer id from its{" "}
            <C>P2P identity</C> log line, and use it in place of <C>PEER_ID</C>:
          </p>
        </Prose>
        <CodeBlock
          label="use your relay"
          code={`python3 -B serve.py \\\n  --relay /ip4/127.0.0.1/tcp/8303/webrtc/http/p2p/PEER_ID`}
        />
        <Prose>
          <p>
            <C>--relay</C> replaces the release&apos;s relays without changing
            the signed files. To dial several relays, give <C>--relay</C> once
            for each. Give each relay as a DNS name or an IPv4 address. The
            server refuses IPv6, because a Content-Security-Policy cannot name
            an IPv6 address. After a change, restart the server and reload the
            page.
          </p>
          <p>
            To set up a relay on another machine, follow{" "}
            <DocLink href="/docs/run-a-relay/#browser-peers">
              Accept web node peers
            </DocLink>
            .
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
