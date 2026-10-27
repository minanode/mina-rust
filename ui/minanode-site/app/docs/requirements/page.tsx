import type { Metadata } from "next";

import { CodeBlock } from "@minanode/design-system/patterns/code-block";
import { KeyValueList } from "@minanode/design-system/patterns/key-value-list";

import { DocsPage } from "@/components/docs/docs-page";
import { C, claimedRows, DocSection, Prose } from "@/components/docs/prose";
import { requirements } from "@/content/docs";

export const metadata: Metadata = {
  title: "Requirements",
  description:
    "The machine minanode needs on Mina's devnet: Linux on x86-64 or ARM64, memory for each role, and disk.",
  alternates: { canonical: "/docs/requirements/" },
};

export default function RequirementsPage() {
  return (
    <DocsPage slug="requirements" title="Requirements" lead={null}>
      <DocSection id="system" title="System">
        <KeyValueList
          align="left"
          title="What it runs on"
          rows={claimedRows(requirements.system)}
        />
        <Prose>
          <p data-claim={requirements.cpu.claim}>{requirements.cpu.text}</p>
        </Prose>
        <CodeBlock
          code="grep -o -w -E 'bmi2|adx' /proc/cpuinfo | sort -u"
          label="CPU check"
        />
        <Prose>
          <p>Check your glibc version on the first line of this output:</p>
        </Prose>
        <CodeBlock code="ldd --version" label="glibc check" />
      </DocSection>

      <DocSection id="memory" title="Memory">
        <div className="grid gap-4">
          <KeyValueList
            align="left"
            title="Memory it uses"
            rows={claimedRows(requirements.memory)}
          />
          <KeyValueList
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
            Mina uses the jemalloc allocator on x86-64 and ARM64. To override
            its options, set <code>_RJEM_MALLOC_CONF</code> before starting
            Mina. For example:
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
    </DocsPage>
  );
}
