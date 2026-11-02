import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";

import { DocsPage } from "@/components/docs/docs-page";
import { C, DocLink, DocSection, Prose } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Run a relay",
  description:
    "Accept native and web node peers on a minanode relay, and keep the dashboard private.",
  alternates: { canonical: "/docs/run-a-relay/" },
};

export default function RelayPage() {
  return (
    <DocsPage
      slug="run-a-relay"
      title="Run a relay"
      lead="A native node relays gossip and serves blocks and ledgers to the peers it connects to. Open its peer port so other nodes can join through it, and open its WebRTC signaling listener to serve web nodes."
    >
      <DocSection id="native-peers" title="Accept native peers">
        <Prose>
          <p>
            Allow inbound TCP port <C>8302</C> in your firewall. Behind a
            router, also forward the port to this machine. In the command below,
            replace the example address with your public IP. Behind NAT,{" "}
            <C>--libp2p-external-ip</C> advertises that address. It does not set
            up port forwarding.
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
            stays the same. Keep the file private, and keep the same work
            directory. If you delete the file, the identity changes at the next
            start. A malformed file or unsafe permissions stop the startup.
          </p>
          <p>
            An explicit <C>--p2p-secret-key</C> or <C>MINA_P2P_SEC_KEY</C> takes
            precedence over the saved key. An encrypted <C>--libp2p-keypair</C>{" "}
            with <C>MINA_LIBP2P_PASS</C> takes precedence over both. Explicit
            keys do not replace the saved key. Never share a work directory
            between nodes, because they would use the same identity and share
            changing node data.
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

      <DocSection id="browser-peers" title="Accept web node peers">
        <Prose>
          <p>
            Web nodes connect over WebRTC. To accept them, turn on the separate
            signaling listener with <C>--webrtc-signal-port</C>, or{" "}
            <C>MINA_WEBRTC_SIGNAL_PORT</C>. The listener&apos;s address defaults
            to <C>127.0.0.1</C>. <C>--webrtc-signal-addr</C>, or{" "}
            <C>MINA_WEBRTC_SIGNAL_ADDR</C>, changes it. The binary needs the{" "}
            <C>p2p-webrtc</C> feature, which the default native build includes.
          </p>
          <p>
            The listener serves only <C>POST /mina/webrtc/signal</C> and{" "}
            <C>GET /mina/webrtc/signal/&#123;offer&#125;</C>, with browser CORS.
            It exposes no dashboard, API, or worker control. Requests have a
            30-second deadline and a 2 MiB body limit.
          </p>
          <p>
            minaweb.org dials only the relays its build lists. Your relay serves
            a{" "}
            <DocLink href="/docs/run-web-node/#relay">
              web node you run yourself
            </DocLink>{" "}
            with <C>--relay</C>. With <C>--webrtc-sharing-relay</C>, a relay
            introduces only browsers that turned sharing on. 0.21.0 removes{" "}
            <C>--webrtc-signaling-relay</C>. Use <C>--webrtc-sharing-relay</C>{" "}
            instead.{" "}
            <DocLink href="/docs/web-node-connections/#sharing">
              Sharing with other browsers
            </DocLink>{" "}
            explains what an introduction exposes.
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
            Signaling exchanges the connection details. WebRTC then uses its own
            UDP connection. Add <C>--webrtc-udp-ports 40000-40999</C>, and allow
            that UDP range through the firewall. Behind a 1:1 NAT, also forward
            the range, and set <C>--webrtc-nat-ip</C> to your public IP.
          </p>
          <p>
            Clients whose networks block direct UDP also need a TURN server.{" "}
            <C>--webrtc-ice-servers</C> reads a JSON file of ICE server settings
            in the browser&apos;s format, including STUN or TURN URLs and
            credentials.
          </p>
          <p>
            A web node on the same machine uses the address below. Its TCP port
            is the signaling listener, separate from the libp2p port. The client
            builds the signaling URL from this host and port.
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
            An HTTPS page needs HTTPS signaling. Caddy can serve it and get its
            own TLS certificate. This Caddyfile also caps signaling bodies at
            128 KiB. Use Caddy only for a relay that serves your own web node,
            as{" "}
            <DocLink href="/docs/run-a-relay/#public-relay">
              Serve the public
            </DocLink>{" "}
            explains.
          </p>
          <p>
            Keep port <C>8303</C> on loopback. Keep the HTTP API and the
            dashboard on their default, <C>127.0.0.1:3000</C>, as{" "}
            <DocLink href="/docs/dashboard/#on-a-server">
              Open it on a server
            </DocLink>{" "}
            explains.
          </p>
          <ol>
            <li>
              Point a DNS name, here <C>signal.example.org</C>, at your server.
            </li>
            <li>
              Allow inbound TCP ports <C>80</C> and <C>443</C>.
            </li>
            <li>
              Save this Caddyfile, with your domain in place of the example, and
              run Caddy with it:
            </li>
          </ol>
        </Prose>
        <CodeBlock
          code={`signal.example.org {
  @signal path /mina/webrtc/signal /mina/webrtc/signal/*
  handle @signal {
    request_body {
      max_size 128KiB
    }
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
            Give this web peer address to <C>--relay</C>. It resolves to{" "}
            <C>https://signal.example.org:443/mina/webrtc/signal</C>. Use the
            same public peer id as in the native address.
          </p>
        </Prose>
        <CodeBlock
          code="/dns4/signal.example.org/tcp/443/webrtc/https/p2p/PEER_ID"
          label="public WebRTC peer address"
        />
        <Prose>
          <p>
            Proxy only the signaling listener. A path prefix at the proxy is
            optional. Clients accept <C>/http-path/URL_ENCODED_PREFIX</C> before{" "}
            <C>/p2p/PEER_ID</C>. If you use a prefix, strip it at the proxy
            before it forwards the request to the listener.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="public-relay" title="Serve the public">
        <Prose>
          <p>
            The Caddy setup above is for a relay that serves your own web node.
            Caddy has no built-in rate or connection limits, so don&apos;t use
            it for a relay that any page can reach.
          </p>
          <p>
            The minaweb.org relays put nginx in front of the signaling listener,
            with per-address rate and connection limits. Each runs its own
            coturn 4.18.0, built from the pinned upstream release tarball, with
            TURN credentials that expire. A firewall rule lets coturn send only
            to that host&apos;s relay node.
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
