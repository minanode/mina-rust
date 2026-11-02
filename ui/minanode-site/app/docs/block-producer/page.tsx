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
import { producer, uptime } from "@/content/docs";

export const metadata: Metadata = {
  title: "Produce blocks",
  description:
    "Produce blocks on Mina's devnet with minanode: the key file and its password, the coinbase receiver, saved slot checks, and the producer page.",
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
      title="Produce blocks"
      lead="With a block producer key, the node checks each slot for a win and produces a block in each slot it wins. The more stake the key has, the more slots it wins."
    >
      <Prose>
        <p>
          The commands on this page use the release binary. For Docker, follow{" "}
          <DocLink href="/docs/docker/#block-producer">
            the Docker page&apos;s block producer section
          </DocLink>
          .
        </p>
      </Prose>
      <DocSection id="key" title="Make a key">
        <Prose>
          <p>
            The key needs stake on devnet. Its stake is its own balance plus
            what others delegate to it. The node counts the stake in the current
            epoch&apos;s staking ledger.
          </p>
          <p>
            To make a new key, choose a password, then write the key file.{" "}
            <C>mina wallet generate</C> reads the password from{" "}
            <C>MINA_PRIVKEY_PASS</C>, and stops if the variable is unset or
            empty. The command prints the public key and saves it next to the
            file, in <C>producer-key.pub</C>.
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
            copy of the file somewhere safe. Keep the file and the password off
            shared machines and out of bug reports.
          </p>
        </Caution>
      </DocSection>

      <DocSection id="when-stake-counts" title="When stake counts">
        <Prose>
          <p>
            A delegation counts only after the epoch changes twice. Stake
            delegated during an epoch enters the next epoch ledger at the first
            change. That ledger becomes the staking ledger at the second change.
            So stake delegated in epoch N counts from epoch N + 2 at the
            earliest.
          </p>
          <p>
            The node takes the snarked ledger at each change. A delegation in
            one of an epoch&apos;s last blocks can wait one epoch more.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="key-permissions" title="Key file permissions">
        <Prose>
          <p>
            An encrypted key file must give access to its owner only, as the
            OCaml node requires. Use mode <C>0600</C>. Mode <C>0400</C> also
            works. The node and the wallet commands refuse a producer, libp2p,
            or wallet key file if the group or other users have any access to
            it. The error gives the file&apos;s mode.
          </p>
          <p>
            To fix the mode, run <C>chmod 600</C> on the key file. Key files
            that <C>mina</C> writes already have mode <C>0600</C>. The file from{" "}
            <C>mina wallet generate</C> is one of them.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="passwords" title="Key passwords">
        <Prose>
          <p>
            <C>mina</C> reads key passwords from the environment, never from the
            command line. <C>MINA_PRIVKEY_PASS</C> holds the password for
            producer and wallet keys. <C>MINA_LIBP2P_PASS</C> holds the password
            for a libp2p key file given with <C>--libp2p-keypair</C>.
          </p>
          <p>
            If you give a password on the command line, <C>mina</C> refuses it
            at startup, with a message that names the variable. When{" "}
            <C>MINA_PRIVKEY_PASS</C> is not set, <C>mina wallet delegate</C>{" "}
            asks for the password on a terminal. The{" "}
            <DocLink href="/docs/cli/#wallet">CLI reference</DocLink> lists the
            wallet commands and their options.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="web-node-key" title="Move a key to or from the web node">
        <Prose>
          <p>
            The web node runs on devnet only. Move only a key you made for
            devnet into it: a Mina key controls the same account on every
            network.
          </p>
          <p>
            To use a native key file in the web node, open its{" "}
            <strong>Your key</strong> page and choose{" "}
            <strong>Import a devnet key instead</strong>. Choose{" "}
            <strong>Key file</strong>, pick the file or paste its text, and type
            its password. The page reads the format that{" "}
            <C>mina wallet generate</C> and the OCaml node write. It refuses a
            file that needs more than 256 MiB of memory to decrypt.
          </p>
          <p>
            The tab holds the imported key in memory. It saves the key in the
            browser, encrypted, only if you use{" "}
            <strong>Save it in this browser</strong>.
          </p>
          <p>
            To use a web node key in a native node, go to{" "}
            <strong>Export a key file</strong> on the <strong>Your key</strong>{" "}
            page. Type a password of at least 10 characters twice, and choose{" "}
            <strong>Download key file</strong>. Run <C>chmod 600</C> on the
            file, then give it to <C>--producer-key</C> as below.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="start" title="Start producing blocks">
        <CodeBlock code={start} label="start command with a producer key" />
        <Prose>
          <p>
            <C>MINA_PRIVKEY_PASS</C> holds the key file&apos;s password.{" "}
            <C>read -s</C> keeps the password off the screen and out of your
            shell history.
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
            don&apos;t start a second node in a terminal. Instead, give the
            service the producer flags with a drop-in file that replaces its
            start command. Keep the password in a file that only you can read:
          </p>
        </Prose>
        <CodeBlock code={service} label="producer flags for the user service" />
        <Prose>
          <p>
            To go back to a plain full node, delete{" "}
            <C>~/.config/systemd/user/minanode.service.d/producer.conf</C>. Then
            run <C>systemctl --user daemon-reload</C>, and restart the service.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="slot-checks" title="Saved slot checks">
        <Prose>
          <p>
            The node checks every slot of the epoch for a win. It saves the
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

      <DocSection id="expected-wins" title="Expected wins">
        <Prose>
          <p>
            Each slot is a draw. A key wins a slot with the chance 1 − (1/4)
            <sup>s</sup>, where s is its share of the epoch&apos;s staking
            ledger. The 3/4 in it is Mina&apos;s active slot coefficient.
          </p>
          <p>
            The wins to expect are that chance times the slots checked. The 95%
            range comes from the binomial distribution: the count of wins found
            lands in it 95 times in 100.
          </p>
          <p>
            A count outside the range is unusual, not proof of a fault. With
            fewer than 5 wins expected, the range says little, and the page says
            so.
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
            The won slots ahead are listed soonest first. Scheduled means the
            node has its attempt ready and waits for the slot.
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
            A restart shortly before a won slot can miss the slot. Check the
            page before you restart the node.
          </p>
        </Prose>
      </DocSection>

      <DocSection id="uptime" title="Delegation program uptime">
        <Prose>
          <p data-claim={uptime.reports.claim}>{uptime.reports.text}</p>
          <p>
            To turn reporting on, set <C>--uptime-url URL</C> and either{" "}
            <C>--uptime-submitter-key FILE</C> or{" "}
            <C>--uptime-submitter-pubkey PUBLICKEY</C>. Set{" "}
            <C>UPTIME_PRIVKEY_PASS</C> privately to the key&apos;s password. The
            variable must be present even when the password is empty. The key
            need not be the producer&apos;s. The key file must have mode 0600.
          </p>
          <p>
            A report holds the block, its creation time, the peer ID, the
            signature, and the submitter. The best-tip report includes selected
            work when a suitable job exists. A scheduled local block also gets a
            report, without work. <C>--uptime-send-node-commit-sha</C> adds the
            build commit. <C>--snarker-fee</C> sets the work fee.
          </p>
          <p>
            To save signed report bodies without sending them, add{" "}
            <C>--uptime-dry-run-dir DIR</C>. A dry run still needs a URL and a
            key, so use a loopback URL for local checks. A dry run still loads
            the circuits and proves work. <C>/reporting/status</C> shows the
            reporter&apos;s results.
          </p>
          <p data-claim={uptime.memory.claim}>{uptime.memory.text}</p>
          <p data-claim={uptime.zkapp.claim}>{uptime.zkapp.text}</p>
          <p>
            If the uptime worker fails, the node skips that report and keeps
            running. Sending reports does not enroll a key in the delegation
            program.
          </p>
          <p>
            The{" "}
            <ExternalLink href="https://github.com/minanode/mina-rust/blob/develop/docs/release/uptime-reporting.md">
              uptime reporting notes
            </ExternalLink>{" "}
            give the intended differences from OCaml, the measurements, and the
            release gate. The{" "}
            <DocLink href="/docs/cli/#node--uptime-url">CLI reference</DocLink>{" "}
            lists the flags and environment variables. The Mina docs describe
            the{" "}
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
