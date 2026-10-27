import type { Metadata } from "next";
import { ArrowRight } from "lucide-react";

import { StatusBadge } from "@minanode/design-system/patterns/status-badge";

import { DocsPage } from "@/components/docs/docs-page";
import { DocSection, Prose } from "@/components/docs/prose";
import { docsIndex, docsPages } from "@/content/docs";

export const metadata: Metadata = {
  title: "Docs",
  description:
    "How to run minanode on Mina's devnet: requirements, install, the dashboard, the SNARK worker, block production and bug reports.",
  alternates: { canonical: "/docs/" },
};

export default function DocsIndex() {
  return (
    <DocsPage
      slug=""
      title="Run a node on devnet"
      lead={
        <span data-claim={docsIndex.lead.claim}>{docsIndex.lead.text}</span>
      }
      aside={
        <>
          <StatusBadge
            status="devnet"
            data-claim={docsIndex.status.claim}
            className="w-fit"
          >
            {docsIndex.status.label}
          </StatusBadge>
          <p data-claim={docsIndex.beta.claim} className="text-foreground">
            {docsIndex.beta.text}
          </p>
        </>
      }
    >
      <div>
        <ul className="grid gap-3 sm:grid-cols-2">
          {docsPages.map(page => (
            <li key={page.slug} className="flex">
              <a
                href={`/docs/${page.slug}/`}
                className="group flex w-full flex-col gap-1 rounded-xl border bg-card p-4 transition-colors duration-150 hover:border-brand-border focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring"
              >
                <span className="inline-flex items-center gap-1.5 font-medium text-foreground">
                  {page.title}
                  <ArrowRight
                    aria-hidden
                    className="size-3.5 text-brand-text transition-transform duration-150 group-hover:translate-x-0.5"
                  />
                </span>
                <span className="text-sm text-muted-foreground">
                  {page.summary}
                </span>
              </a>
            </li>
          ))}
        </ul>
      </div>
      <DocSection id="known-limitations" title="Known limitations">
        <Prose>
          <ul data-claim={docsIndex.limitations.claim}>
            {docsIndex.limitations.items.map(item => (
              <li key={item}>{item}</li>
            ))}
          </ul>
        </Prose>
      </DocSection>
    </DocsPage>
  );
}
