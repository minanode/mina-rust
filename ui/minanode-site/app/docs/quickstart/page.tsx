import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";

import { DocsPage } from "@/components/docs/docs-page";
import { C, DocLink, DocSection, Prose } from "@/components/docs/prose";
import { sshTunnel } from "@/content/docs";
import {
  archiveName,
  release,
  releaseSignerUrl,
  releaseSigningFingerprint,
  verifyReleaseCommand,
} from "@/content/release";

export const metadata: Metadata = {
  title: "Quickstart",
  description: "Download minanode, run it on devnet and open the dashboard.",
  alternates: { canonical: "/docs/quickstart/" },
};

const download = `# Choose x86_64 or arm64 for your Linux CPU.
arch=x86_64
url=${release.downloadUrl}
curl -fLO "$url/${archiveName("$arch")}" &&
curl --fail-early -fLO "$url/SHA256SUMS" \\
  -O "$url/SHA256SUMS.sig" -O ${releaseSignerUrl} &&
${verifyReleaseCommand} &&
sha256sum --check --ignore-missing SHA256SUMS`;

export default function QuickstartPage() {
  return (
    <DocsPage
      slug="quickstart"
      title="Quickstart"
      lead="In this quickstart, we download the release, check it, start a devnet node, and open its dashboard."
    >
      <DocSection id="download" title="1. Download and check">
        <Prose>
          <p>
            The archives are for Linux. On macOS or Windows,{" "}
            <DocLink href="/docs/docker/">run the node in Docker</DocLink>{" "}
            instead. On Apple silicon, <C>uname -m</C> also prints <C>arm64</C>,
            but the archive does not run on macOS.
          </p>
          <p>
            These commands need <C>curl</C>, <C>tar</C>, <C>sha256sum</C>, and
            OpenSSH 8.2 or newer.{" "}
            <DocLink href="/docs/requirements/#install-tools">
              Tools for installing
            </DocLink>{" "}
            lists where they come from.
          </p>
          <p>
            First, set <C>arch</C> to your CPU and run the commands. They
            download the archive, the checksum list, its signature, and the
            release signer file. Then they verify the signature and check the
            archive.
          </p>
        </Prose>
        <CodeBlock code={download} label="download and checksum commands" />
        <Prose>
          <p>
            The output includes a line that starts with{" "}
            <C>Good &quot;file&quot; signature for release@minanode.org</C>,
            then the archive&apos;s name followed by <C>OK</C>.
          </p>
          <p data-claim="C81">
            Now check that the signer file holds the release key. This command
            prints the key&apos;s fingerprint, which must be{" "}
            <C>{releaseSigningFingerprint}</C>. To confirm a new or changed key,
            follow{" "}
            <DocLink href="/docs/install/#check">
              Verify the signature and the checksum
            </DocLink>
            .
          </p>
        </Prose>
        <CodeBlock
          code="ssh-keygen -lf allowed_signers -E sha256"
          label="release key fingerprint"
        />
        <Prose>
          <p data-claim="C69">
            Continue only if all three checks pass: a good signature, an{" "}
            <C>OK</C> checksum, and a matching fingerprint.
          </p>
        </Prose>
      </DocSection>
      <DocSection id="run" title="2. Unpack and run">
        <Prose>
          <p>Next, we unpack the archive and start the node on devnet.</p>
        </Prose>
        <CodeBlock
          code={`mkdir mina-${release.version}
tar -xzf "${archiveName("$arch")}" -C mina-${release.version}
cd mina-${release.version}
./mina node --network devnet`}
          label="unpack and run commands"
        />
        <Prose>
          <p>
            The node logs to the terminal. It finds peers, syncs, and then
            follows the tip of the chain.
          </p>
          <p>
            The node runs in the foreground, so it stops when you close the
            terminal. To keep it running,{" "}
            <DocLink href="/docs/run-a-node/#service">
              run it in the background
            </DocLink>
            .
          </p>
        </Prose>
      </DocSection>
      <DocSection id="dashboard" title="3. Open the dashboard">
        <Prose>
          <p>
            Last, open{" "}
            <DocLink href="http://127.0.0.1:3000">
              http://127.0.0.1:3000
            </DocLink>
            . The Overview page shows the sync state, the best tip, and the
            peers.
          </p>
          <p>
            If the node runs on a server, first run this tunnel on your own
            computer.{" "}
            <DocLink href="/docs/dashboard/#on-a-server">
              Open it on a server
            </DocLink>{" "}
            explains the tunnel.
          </p>
        </Prose>
        <CodeBlock code={sshTunnel} label="SSH tunnel command" />
      </DocSection>
      <DocSection id="next" title="Next steps">
        <Prose>
          <p>You now have a devnet full node. From here, you can:</p>
          <ul>
            <li>
              <DocLink href="/docs/install/#run">
                install the binary in <C>~/.local/bin</C>
              </DocLink>
            </li>
            <li>
              <DocLink href="/docs/run-a-node/#service">
                run the node in the background
              </DocLink>
            </li>
            <li>
              <DocLink href="/docs/snark-worker/">run a SNARK worker</DocLink>
            </li>
            <li>
              <DocLink href="/docs/block-producer/">produce blocks</DocLink>
            </li>
          </ul>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
