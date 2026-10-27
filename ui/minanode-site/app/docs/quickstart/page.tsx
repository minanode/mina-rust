import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";

import { DocsPage } from "@/components/docs/docs-page";
import { C, DocLink, DocSection, Prose } from "@/components/docs/prose";
import {
  archiveName,
  release,
  releaseSignerUrl,
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
    <DocsPage slug="quickstart" title="Quickstart" lead={null}>
      <DocSection id="download" title="1. Download and check">
        <Prose>
          <p>
            The archives are for Linux. On macOS or Windows,{" "}
            <DocLink href="/docs/docker/">run the node in Docker</DocLink>.
            macOS on Apple silicon also says <C>arm64</C> to <C>uname -m</C>,
            but the archive will not run there.
          </p>
          <p data-claim="C69">
            Continue only after a good signature and an <C>OK</C> checksum.{" "}
            <DocLink href="/docs/install/#check">Verification details</DocLink>.
          </p>
        </Prose>
        <CodeBlock code={download} label="download and checksum commands" />
      </DocSection>
      <DocSection id="run" title="2. Unpack and run">
        <CodeBlock
          code={`mkdir mina-${release.version}
tar -xzf "${archiveName("$arch")}" -C mina-${release.version}
cd mina-${release.version}
./mina node --network devnet`}
          label="unpack and run commands"
        />
        <Prose>
          <p>
            The node runs in the foreground and stops when you close the
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
            Open{" "}
            <DocLink href="http://127.0.0.1:3000">
              http://127.0.0.1:3000
            </DocLink>
            . On a server, first run this tunnel on your computer.
          </p>
        </Prose>
        <CodeBlock
          code="ssh -N -L 3000:127.0.0.1:3000 you@server"
          label="SSH tunnel command"
        />
        <Prose>
          <p>
            Next: <DocLink href="/docs/install/">install</DocLink>,{" "}
            <DocLink href="/docs/run-a-node/#service">
              run in the background
            </DocLink>
            , <DocLink href="/docs/snark-worker/">SNARK worker</DocLink>,{" "}
            <DocLink href="/docs/block-producer/">block producer</DocLink>.
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
