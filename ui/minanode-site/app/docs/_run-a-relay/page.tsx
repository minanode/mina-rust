import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";

import { DocsPage } from "@/components/docs/docs-page";
import { C, DocLink, DocSection, Prose } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Run a relay",
  description:
    "Accept native and browser peers while keeping the dashboard private.",
  alternates: { canonical: "/docs/run-a-relay/" },
};

export default function RelayPage() {
  return (
    <DocsPage
      slug="run-a-relay"
      title="Run a relay"
      lead="A native node relays gossip and serves blocks and ledgers to connected peers. Open its peer port to help other nodes join."
    >
      <DocSection id="native-peers" title="Accept native peers">
        <Prose>
          <p>
            Allow inbound TCP port <C>8302</C> in your firewall and, behind a
            router, forward it to this machine. Replace the example address with
            your public IP. Behind NAT, <C>--libp2p-external-ip</C> advertises
            that address; it does not configure port forwarding.
          </p>
        </Prose>
        <CodeBlock
          code={`mina node \\
  --network devnet -d ~/.mina-relay \\
  --libp2p-port 8302 \\
  --libp2p-external-ip 203.0.113.10`}
          label="start a native relay"
        />
      </DocSection>

      <DocSection id="identity" title="Keep and publish your peer id">
        <Prose>
          <p>
            The first start creates <C>p2p-secret-key</C> in the work directory,
            with mode <C>0600</C> on Unix. Later starts load it, so the peer id
            stays the same. Keep the file private and keep the same work
            directory. Deleting the file changes the identity on the next start.
            A malformed file or unsafe permissions stop startup.
          </p>
          <p>
            An explicit <C>--p2p-secret-key</C> or <C>MINA_P2P_SEC_KEY</C> wins
            over the saved key. An encrypted <C>--libp2p-keypair</C> with{" "}
            <C>MINA_LIBP2P_PASS</C> wins over those. Explicit keys do not
            replace the saved key. Never share a work directory between nodes:
            they would use the same identity and share mutable node data.
          </p>
          <p>
            Read the public peer id from the startup log. Never read or publish
            the key file.
          </p>
        </Prose>
        <CodeBlock
          code={`rg 'P2P identity' ~/.mina-relay/mina.log`}
          label="read the public peer id"
        />
        <Prose>
          <p>Publish this native peer address, replacing the IP and peer id:</p>
        </Prose>
        <CodeBlock
          code="/ip4/203.0.113.10/tcp/8302/p2p/PEER_ID"
          label="native peer address"
        />
      </DocSection>

      <DocSection id="browser-peers" title="Accept browser peers">
        <Prose>
          <p>
            Browser nodes connect over WebRTC. Enable the separate signaling
            listener with <C>--webrtc-signal-port</C> (
            <C>MINA_WEBRTC_SIGNAL_PORT</C>). Its address defaults to{" "}
            <C>127.0.0.1</C>; <C>--webrtc-signal-addr</C> (
            <C>MINA_WEBRTC_SIGNAL_ADDR</C>) changes it. The binary needs the{" "}
            <C>p2p-webrtc</C> feature, included in the default native build.
          </p>
          <p>
            The listener serves only <C>POST /mina/webrtc/signal</C> and{" "}
            <C>GET /mina/webrtc/signal/&#123;offer&#125;</C>, with browser CORS.
            It exposes no dashboard, API or worker control. Requests have a
            30-second deadline and a 2 MiB body limit.
          </p>
        </Prose>
        <CodeBlock
          code={`mina node --network devnet -d ~/.mina-relay \\
  --libp2p-port 8302 --libp2p-external-ip 203.0.113.10 \\
  --webrtc-signal-addr 127.0.0.1 --webrtc-signal-port 8303`}
          label="relay with a private signaling upstream"
        />
        <Prose>
          <p>
            Signaling exchanges connection details; WebRTC then uses its own UDP
            connection. Use <C>--webrtc-udp-ports 40000-40999</C> and allow that
            UDP range through the firewall. Behind a 1:1 NAT, forward it too and
            set <C>--webrtc-nat-ip</C> to your public IP. Clients whose networks
            block direct UDP also need a TURN server.{" "}
            <C>--webrtc-ice-servers</C> reads a JSON file of browser-format ICE
            server settings, including STUN or TURN URLs and credentials.
          </p>
          <p>
            For local HTTP testing, publish the address below. Its TCP port is
            the signaling listener, separate from the libp2p port. The client
            constructs the signaling URL from this host and port.
          </p>
        </Prose>
        <CodeBlock
          code="/ip4/127.0.0.1/tcp/8303/webrtc/http/p2p/PEER_ID"
          label="local WebRTC peer address"
        />
      </DocSection>

      <DocSection id="https" title="Put signaling behind HTTPS">
        <Prose>
          <p>
            An HTTPS page needs HTTPS signaling. Point <C>signal.example.org</C>{" "}
            at your server, allow inbound TCP ports <C>80</C> and <C>443</C>,
            and run Caddy with this Caddyfile. Replace the domain with yours.
            Caddy obtains and renews its TLS certificate. Keep port <C>8303</C>{" "}
            on loopback, and the HTTP API and dashboard on their default{" "}
            <C>127.0.0.1:3000</C> (see{" "}
            <DocLink href="/docs/dashboard/#on-a-server">
              viewing it on a server
            </DocLink>
            ).
          </p>
        </Prose>
        <CodeBlock
          code={`signal.example.org {
  @signal path /mina/webrtc/signal /mina/webrtc/signal/*
  handle @signal {
    reverse_proxy 127.0.0.1:8303
  }
  handle {
    respond "Not found" 404
  }
}`}
          label="Caddyfile"
        />
        <CodeBlock
          code="caddy run --config ./Caddyfile --adapter caddyfile"
          label="run Caddy"
        />
        <Prose>
          <p>
            Publish this browser peer address. It resolves to{" "}
            <C>https://signal.example.org:443/mina/webrtc/signal</C>. Use the
            same public peer id as the native address.
          </p>
        </Prose>
        <CodeBlock
          code="/dns4/signal.example.org/tcp/443/webrtc/https/p2p/PEER_ID"
          label="public WebRTC peer address"
        />
        <Prose>
          <p>
            Proxy only the signaling listener. A proxy prefix is optional:
            clients support <C>/http-path/URL_ENCODED_PREFIX</C> before{" "}
            <C>/p2p/PEER_ID</C>; strip that prefix at the proxy before
            forwarding to the listener.
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
