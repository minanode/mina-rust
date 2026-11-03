import type { Metadata } from "next";
import { DocsPage } from "@/components/docs/docs-page";
import { C, DocLink, DocSection, Prose, Terms } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "How the web node works",
  description:
    "How the Mina web node reaches the network through relays, how it syncs, why a tab runs one role at a time, what sharing with other browsers costs, and who sees your address.",
  alternates: { canonical: "/docs/web-node-connections/" },
};

export default function WebNodeConnectionsPage() {
  return (
    <DocsPage
      slug="web-node-connections"
      title="How the web node works"
      lead="The web node runs in a browser tab. It reaches the Mina network through relays, which are native nodes that browsers can reach over WebRTC."
    >
      <DocSection id="relays" title="Relays connect browsers to the network">
        <Prose>
          <p>
            A browser can only use WebRTC. It can&apos;t open a TCP connection
            or speak libp2p, so it can&apos;t connect to an OCaml node. The
            native node speaks both. It uses libp2p with other nodes and WebRTC
            with browsers. Blocks pass from the Mina network to a relay, and
            then on to the browser.
          </p>
          <p>
            A relay is any native node that browsers can reach. It needs three
            things:
          </p>
          <ul>
            <li>
              its signaling endpoint, over HTTPS for a public page:{" "}
              <C>--webrtc-signal-port</C>
            </li>
            <li>
              its WebRTC UDP ports open: <C>--webrtc-udp-ports</C>
            </li>
            <li>
              a public address, or <C>--webrtc-nat-ip</C> behind a 1:1 NAT
            </li>
          </ul>
          <p>
            The web node&apos;s page lists its relays and dials each one. Its
            Peers page shows each relay, connected or not.{" "}
            <DocLink href="/docs/run-a-relay/">Run a relay</DocLink> sets one
            up.
          </p>
        </Prose>
      </DocSection>
      <DocSection id="connect" title="How a connection starts">
        <Prose>
          <ol>
            <li>
              The browser sends a WebRTC offer to the relay&apos;s signaling
              endpoint over HTTPS, and gets an answer back. The offer and the
              answer describe each side&apos;s network addresses.
            </li>
            <li>The two sides then talk directly over WebRTC.</li>
          </ol>
          <p>Two kinds of server help a connection start:</p>
        </Prose>
        <Terms
          items={[
            {
              term: "STUN",
              text: "Lets the browser learn its own public address. The STUN server sees the browser's IP address. No data passes through it.",
            },
            {
              term: "TURN",
              text: "Carries the traffic when a network blocks the direct route, as some offices, VPNs, and carriers do. A relay's TURN server forwards only to that relay.",
            },
          ]}
        />
        <Prose>
          <p>
            Most connections need no TURN, because each relay has a public
            address. The Peers page shows whether each relay&apos;s route is
            direct or goes through TURN.
          </p>
        </Prose>
      </DocSection>
      <DocSection id="sync" title="How it syncs">
        <Prose>
          <ol>
            <li>
              Over that connection, the web node downloads the staking ledgers
              and the recent blocks.
            </li>
            <li>
              It checks the best tip&apos;s proof itself, and fetches the blocks
              behind the tip by hash.
            </li>
            <li>
              It then follows new blocks as they are gossiped, and checks each
              one&apos;s proof.
            </li>
          </ol>
          <p>
            A relay can stall the web node, but can&apos;t feed it a fake chain.
          </p>
          <p>
            With several relays, the web node times each relay&apos;s answers
            and sends most of its requests to the nearest one. The staged
            ledger, one download of about 35 MiB, goes to the nearest relay too.
            A farther relay gets one request at a time, so it is ready to take
            over if the nearest one stops.
          </p>
        </Prose>
      </DocSection>
      <DocSection id="saved-chain" title="The saved chain">
        <Prose>
          <p>
            The web node saves its ledgers and recent blocks in the
            browser&apos;s storage. After a reload, it restores the chain from
            that copy, then catches up from its relays.
          </p>
          <p>
            The node saves the chain shortly after it reaches Synced. A reload
            before that starts the sync again.
          </p>
          <p>
            The Overview and Start-up pages show what a reload does with the
            saved chain. The node decides it with the same rules it applies when
            it restores.
          </p>
        </Prose>
        <Terms
          items={[
            {
              term: "Not saved yet",
              text: "No chain is saved. A reload starts the sync again.",
            },
            {
              term: "Saved at #N",
              text: "A reload restores the chain up to block N. If a saved file fails the node's checks, the node syncs again instead.",
            },
            {
              term: "Saved, but a reload syncs again",
              text: "The saved chain can't be restored. The Start-up page gives the reason, such as a saved tip more than 1,160 slots old. The node reuses the saved ledgers, syncs, and saves a new chain.",
            },
            {
              term: "Checking",
              text: "The node is restoring the saved chain.",
            },
            {
              term: "Not known",
              text: "The node couldn't read what it needs to decide. The Start-up page gives the reason.",
            },
          ]}
        />
        <Prose>
          <p>
            One tab per browser uses the saved chain. A second tab of the same
            browser runs a second node without it, which uses as much memory
            again. Close one of the tabs to stop its node.
          </p>
          <p>
            The reset on the Start-up page deletes the saved chain and syncs
            again.
          </p>
        </Prose>
      </DocSection>
      <DocSection
        id="not-checked"
        title="Why bootstrap blocks show Not checked"
      >
        <Prose>
          <p>
            On the Blocks page, the blocks from the bootstrap show{" "}
            <strong>Not checked</strong>. The node fetched them by hash, back
            from a best tip whose proof it checked. It did not check each of
            those blocks&apos; own proofs.
          </p>
          <p>
            Each block that arrives after the bootstrap has its own proof
            checked.
          </p>
        </Prose>
      </DocSection>
      <DocSection id="one-role-per-tab" title="One role per tab">
        <Prose>
          <p>
            A web node runs the SNARK worker or the block producer, not both.
            Choosing one role stops the other.
          </p>
          <p>
            The reason is the block&apos;s deadline. A SNARK proof can&apos;t be
            stopped for a block proof. If the tab were proving SNARK work when
            it won a slot, the block proof would wait, and the block could miss
            its slot.
          </p>
          <p>
            A native node can run both roles. It stops taking new worker jobs
            before a known won slot, as{" "}
            <DocLink href="/docs/snark-worker/#flags">
              Start the worker with flags
            </DocLink>{" "}
            describes for <C>--snarker-block-first-lead</C>.
          </p>
        </Prose>
      </DocSection>
      <DocSection id="your-key" title="Your key">
        <Prose>
          <p>
            The Key page makes a key in the browser, from its random number
            generator, or imports one. The key stays in the tab&apos;s memory,
            and the page never sends it anywhere. The web node signs payments in
            the tab, in its WebAssembly, and sends them to the network through
            its relays.
          </p>
          <p>
            To keep a key in the browser, save it with a passphrase. The page
            derives an AES-256-GCM key from the passphrase with PBKDF2-SHA-256
            (600,000 iterations), and stores only the encrypted key, in this
            browser&apos;s storage. Anyone with the device and the passphrase
            can use the key.
          </p>
        </Prose>
      </DocSection>
      <DocSection id="worker-fees" title="Where SNARK worker fees go">
        <Prose>
          <p>
            The SNARK worker page fills in the key held or saved in the tab as
            the fee recipient. You can enter any other public key instead. A new
            key has no account on chain, so its fee must be at least 1 MINA. A
            key with an account can ask any fee.{" "}
            <DocLink href="/docs/snark-worker/#fee-recipient">
              Choose the fee recipient
            </DocLink>{" "}
            has the details.
          </p>
        </Prose>
      </DocSection>
      <DocSection id="sharing" title="Sharing with other browsers">
        <Prose>
          <p>
            Sharing lets other visitors&apos; web nodes sync from yours. It is
            off by default. Turn it on or off on the Peers page, and this
            browser keeps the choice. Either way, the node restarts, and a key
            or role that the tab holds carries over.
          </p>
          <p>With sharing on, the web node behaves as follows:</p>
          <ul>
            <li>
              The page still dials only its relays. It asks them to introduce it
              to other browsers that turned sharing on.
            </li>
            <li>
              It connects to at most 4 browsers, and helps at most 2 of them
              sync at once.
            </li>
            <li>
              A browser it meets can&apos;t point it at a private address on
              your network. That browser can name public addresses, and your
              browser tries those.
            </li>
            <li>
              A relay introduces only pages that turned sharing on. Each relay
              permits at most 6 introductions per peer key per hour, 12 per IPv4
              address or IPv6 /64 per hour, and 120 in total per hour.
            </li>
          </ul>
          <h3>What sharing costs you</h3>
          <ul>
            <li>
              The browsers you connect to see your public IP address: IPv4, and
              IPv6 if you have it. An introduction exchanges WebRTC offers, and
              the offers hold each browser&apos;s addresses.
            </li>
            <li>
              Anyone who runs a modified page with sharing on sees it too. Such
              a page can keep asking for introductions, and record each address
              it gets, as fast as the relays&apos; limits allow.
            </li>
            <li>
              The relay can&apos;t read the offers, because they are encrypted.
              The relay does see which browsers it introduced to each other.
            </li>
            <li>
              Sharing uses your upload. Each tab that you help sync takes up to
              about 130 MiB. Each connected browser then takes tens of MiB an
              hour, in each direction.
            </li>
          </ul>
          <p>
            To stop, turn the switch off on the Peers page. The node restarts
            without sharing, and drops its browser connections.
          </p>
          <p>
            With <C>--webrtc-sharing-relay</C>, a relay introduces only browsers
            that turned sharing on. 0.21.0 removes{" "}
            <C>--webrtc-signaling-relay</C>. Use <C>--webrtc-sharing-relay</C>{" "}
            instead. A relay without <C>--webrtc-sharing-relay</C> introduces no
            one, and sharing then finds no browsers.
          </p>
        </Prose>
      </DocSection>
      <DocSection id="who-sees-what" title="Who sees your address">
        <Prose>
          <p>
            Each relay you connect to sees your IP address. So do the STUN and
            TURN servers that the page uses, and the hosts that its files come
            from. With sharing on, so do the browsers you connect to. On
            minaweb.org, the STUN and TURN servers run on the relays&apos;
            hosts.
          </p>
          <p>
            To use your own relay instead,{" "}
            <DocLink href="/docs/run-web-node/">
              run the web node yourself
            </DocLink>{" "}
            and point its <C>--relay</C> option at your own node.
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
