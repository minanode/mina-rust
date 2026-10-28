import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";

import { DocsPage } from "@/components/docs/docs-page";
import {
  C,
  Caution,
  DocLink,
  DocSection,
  Prose,
} from "@/components/docs/prose";
import { producer } from "@/content/docs";

export const metadata: Metadata = {
  title: "Block producer",
  description:
    "Produce blocks on Mina's devnet with minanode: the key file and its password, the coinbase receiver, saved slot checks and the producer page.",
  alternates: { canonical: "/docs/block-producer/" },
};

const start = `printf "Key password: " && IFS= read -rs MINA_PRIVKEY_PASS && echo &&
  export MINA_PRIVKEY_PASS
mina node --network devnet \\
  --producer-key ~/keys/producer-key`;

const service = `systemctl --user stop minanode.service
(umask 077 && printf "Key password: " && IFS= read -rs P && echo &&
  printf "MINA_PRIVKEY_PASS='%s'\\n" "$P" > ~/keys/producer.env)
mkdir -p ~/.config/systemd/user/minanode.service.d
cat > ~/.config/systemd/user/minanode.service.d/producer.conf <<'EOF'
[Service]
EnvironmentFile=%h/keys/producer.env
ExecStart=
ExecStart=%h/.local/bin/mina node --network devnet \\
  --disable-filesystem-logging --producer-key %h/keys/producer-key
EOF
systemctl --user daemon-reload
systemctl --user start minanode.service`;

export default function ProducerPage() {
  return (
    <DocsPage
      slug="block-producer"
      title="Block producer"
      lead="With a block producer key, the node checks each slot for a win and produces a block in each slot it wins. The more stake the key has, the more slots it wins."
    >
      <Prose>
        <p>
          The commands on this page use the release binary. In Docker, see{" "}
          <DocLink href="/docs/docker/#block-producer">
            the Docker page&apos;s block producer section
          </DocLink>
          .
        </p>
      </Prose>
      <DocSection id="key" title="The key">
        <Prose>
          <p>
            The key needs stake on devnet: its own balance, plus what others
            delegate to it. The node counts the stake in the current
            epoch&apos;s staking ledger.
          </p>
          <p>
            To make a new key, choose a password, then write the key file.{" "}
            <C>mina wallet generate</C> reads the password from{" "}
            <C>MINA_PRIVKEY_PASS</C> and stops if it is unset or empty. The
            command prints the public key and saves it next to the file, in{" "}
            <C>producer-key.pub</C>.
          </p>
        </Prose>
        <CodeBlock
          code={`mkdir -p -m 700 ~/keys
printf "Key password: " && IFS= read -rs MINA_PRIVKEY_PASS && echo &&
  export MINA_PRIVKEY_PASS
mina wallet generate --output ~/keys/producer-key`}
          label="key commands"
        />
        <Caution title="Keep the key file safe">
          <p>
            The file holds your private key, encrypted with the password. Keep a
            copy somewhere safe, and keep it and the password off shared
            machines and out of bug reports.
          </p>
        </Caution>
      </DocSection>

      <DocSection id="key-permissions" title="Key file permissions">
        <Prose>
          <p>
            An encrypted key file must be accessible by its owner only, as the
            OCaml node requires. Use mode <C>0600</C>; <C>0400</C> also works.
            The node and the wallet commands refuse a producer, libp2p or wallet
            key file that group or other users have any access to. The error
            gives the file&apos;s mode.
          </p>
          <p>
            To fix it, run <C>chmod 600</C> on the key file. Key files that{" "}
            <C>mina</C> writes, such as the one from <C>mina wallet generate</C>
            , already have mode <C>0600</C>.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="passwords" title="Key passwords">
        <Prose>
          <p>
            Key passwords are read from the environment, never from the command
            line. <C>MINA_PRIVKEY_PASS</C> holds the password for producer and
            wallet keys. <C>MINA_LIBP2P_PASS</C> holds the password for a libp2p
            key file given with <C>--libp2p-keypair</C>. A password given on the
            command line is refused at startup, with a message naming the
            variable. <C>mina wallet delegate</C> also asks for the password on
            a terminal when <C>MINA_PRIVKEY_PASS</C> is not set. The{" "}
            <DocLink href="/docs/cli/#wallet">CLI reference</DocLink> lists the
            wallet commands and their options.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="start" title="Start producing">
        <CodeBlock code={start} label="start command with a producer key" />
        <Prose>
          <p>
            <C>MINA_PRIVKEY_PASS</C> holds the key file&apos;s password. Reading
            it with <C>read -s</C> keeps it off the screen and out of your shell
            history.
          </p>
          <p>
            Block rewards go to the producer key. To send them to another key,
            add <C>--coinbase-receiver B62q...</C>. The{" "}
            <DocLink href="/docs/cli/#node">CLI reference</DocLink> lists the
            other block production options.
          </p>
          <p>
            A block producer keeps proofs in memory, not in the{" "}
            <DocLink href="/docs/run-a-node/#proof-cache">proof cache</DocLink>.{" "}
            <C>--proof-cache</C> turns the cache on.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="service" title="Under the user service">
        <Prose>
          <p>
            If the node runs as the{" "}
            <DocLink href="/docs/run-a-node/#service">user service</DocLink>,
            don&apos;t start a second one in a terminal. Give the service the
            producer flags instead, with a drop-in file that replaces its start
            command, and keep the password in a file only you can read:
          </p>
        </Prose>
        <CodeBlock code={service} label="producer flags for the user service" />
        <Prose>
          <p>
            To go back to a plain full node, delete{" "}
            <C>~/.config/systemd/user/minanode.service.d/producer.conf</C>, then
            run <C>systemctl --user daemon-reload</C> and restart the service.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="slot-checks" title="Saved slot checks">
        <Prose>
          <p>
            The node checks every slot of the epoch for a win and saves the
            results in <C>vrf-evaluation/</C> in its work directory, so a
            restart doesn&apos;t check them again.{" "}
            <span data-claim={producer.slotChecks.claim}>
              {producer.slotChecks.text}
            </span>
          </p>
          <p>
            To make it check again, stop the node, delete <C>vrf-evaluation/</C>{" "}
            and start it.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="producer-page" title="The producer page">
        <Prose>
          <p>
            The dashboard&apos;s Block producer page is read-only. It shows the
            producer key, this epoch&apos;s slot checks and the won slots ahead.
          </p>
          <p>
            For each past won slot, the page shows its outcome: canonical,
            orphaned, lost the tie to another block for the same slot, or
            missed, with the reason. It also shows when the block was built,
            proved, applied and sent, against the 90 s slot.
          </p>
          <p>
            A summary for the epoch, and since the start, counts the outcomes,
            gives the median and worst proof times and the coinbase earned. It
            also sets the wins found against the wins the key&apos;s stake
            should bring.
          </p>
          <p>
            The node keeps its newest 512 won slots and 16 epochs of slot checks
            in <C>block-production.json</C> in its work directory, readable by
            you only, so the page keeps them across a restart.
          </p>
          <p>
            A restart shortly before a won slot may miss the slot, so check the
            page before restarting the node.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="uptime" title="Delegation program uptime">
        <Prose>
          <p>
            0.20.0 doesn&apos;t send uptime reports to the uptime service of the
            Mina Foundation delegation program. Support is coming soon. If you
            need delegation program uptime, keep an OCaml node running for it
            for now. The Mina docs describe the{" "}
            <ExternalLink href="https://docs.minaprotocol.com/node-operators/delegation-program/uptime-tracking-system">
              uptime tracking system
            </ExternalLink>
            .
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
