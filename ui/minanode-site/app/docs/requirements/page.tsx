import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";

import { DocsPage } from "@/components/docs/docs-page";
import {
  C,
  claimedRows,
  DocLink,
  DocSection,
  Prose,
} from "@/components/docs/prose";
import { requirements } from "@/content/docs";

export const metadata: Metadata = {
  title: "Requirements",
  description:
    "The machine minanode needs on Mina's devnet: Linux on x86-64 or ARM64, memory for each role, and disk.",
  alternates: { canonical: "/docs/requirements/" },
};

export default function RequirementsPage() {
  return (
    <DocsPage
      slug="requirements"
      title="Requirements"
      lead="The systems the node runs on, the memory and disk it uses in each role, and the tools that installing it needs."
    >
      <DocSection id="system" title="System">
        <KeyValueList
          variant="receipt"
          align="left"
          title="What it runs on"
          rows={claimedRows(requirements.system)}
        />
        <Prose>
          <p>
            The CPU and glibc checks below apply to the Linux archives. On macOS
            and Windows, the node runs in Docker, as{" "}
            <DocLink href="/docs/docker/#macos-and-windows">
              macOS and Windows
            </DocLink>{" "}
            on the Docker page describes. The Docker image has its own C
            library, so the glibc check does not apply to it.
          </p>
          <p data-claim={requirements.cpu.claim}>{requirements.cpu.text}</p>
        </Prose>
        <CodeBlock
          code="grep -o -w -E 'bmi2|adx' /proc/cpuinfo | sort -u"
          label="CPU check"
        />
        <Prose>
          <p>The first line of this output gives the glibc version:</p>
        </Prose>
        <CodeBlock code="ldd --version" label="glibc check" />
      </DocSection>

      <DocSection id="memory" title="Memory">
        <div className="grid gap-4">
          <KeyValueList
            variant="receipt"
            align="left"
            title="Memory it uses"
            rows={claimedRows(requirements.memory)}
          />
          <KeyValueList
            variant="receipt"
            align="left"
            title="Suggested machine"
            rows={claimedRows(requirements.machine)}
          />
        </div>
        <Prose>
          <p data-claim={requirements.memoryNote.claim}>
            {requirements.memoryNote.text}
          </p>
          <p>
            <C>mina</C> uses the jemalloc allocator on x86-64 and ARM64.{" "}
            <C>_RJEM_MALLOC_CONF</C>, set before <C>mina</C> starts, overrides
            the allocator&apos;s options. For example:
          </p>
        </Prose>
        <CodeBlock
          code="export _RJEM_MALLOC_CONF='narenas:8'"
          label="Allocator override"
        />
      </DocSection>

      <DocSection id="disk" title="Disk">
        <Prose>
          <p data-claim={requirements.disk.claim}>{requirements.disk.text}</p>
        </Prose>
      </DocSection>

      <DocSection id="install-tools" title="Tools for installing">
        <Prose>
          <p>Downloading and checking a release archive needs these tools:</p>
          <ul>
            <li>
              <C>curl</C> and <C>tar</C>
            </li>
            <li>
              <C>sha256sum</C>, from GNU coreutils
            </li>
            <li>
              OpenSSH 8.2 or newer, for <C>ssh-keygen -Y verify</C>
            </li>
          </ul>
          <p>
            <DocLink href="/docs/docker/">Run with Docker</DocLink> needs Docker
            Engine or Docker Desktop, and a user that can run it. Its Compose
            section also needs <C>curl</C> and the Compose plugin, which
            provides <C>docker compose</C>.
          </p>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
