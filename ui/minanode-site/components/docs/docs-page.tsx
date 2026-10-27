import * as React from "react";
import { ArrowLeft, ArrowRight, ChevronDown } from "lucide-react";

import { cn } from "@minanode/design-system/lib/utils";
import { Eyebrow } from "@minanode/design-system/patterns/eyebrow";

import { Container } from "@/components/container";
import { docsPages, type DocsPage as Page } from "@/content/docs";

const index = { slug: "", title: "Docs overview" };

function href(page: { slug: string }) {
  return page.slug ? `/docs/${page.slug}/` : "/docs/";
}

const focusRing =
  "focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring";

function NavList({ current }: { current: string }) {
  return (
    <ul className="flex flex-col gap-0.5">
      {[index, ...docsPages].map(page => {
        const active = page.slug === current;
        return (
          <li key={page.slug}>
            <a
              href={href(page)}
              aria-current={active ? "page" : undefined}
              className={cn(
                "block rounded-md px-3 py-2.5 text-sm lg:py-2 transition-colors duration-150",
                focusRing,
                active
                  ? "bg-brand-subtle font-medium text-brand-text"
                  : "text-muted-foreground hover:bg-accent hover:text-foreground",
              )}
            >
              {page.title}
            </a>
          </li>
        );
      })}
    </ul>
  );
}

/** The Docs pages: a sticky list from lg up, a disclosure above the page
 * below lg. Only one is displayed at a time, so there is one Docs landmark. */
function DocsNav({ current }: { current: string }) {
  const title =
    docsPages.find(page => page.slug === current)?.title ?? index.title;
  return (
    <>
      <nav aria-label="Docs" className="hidden lg:block">
        <div className="sticky top-24 flex flex-col gap-3">
          <Eyebrow className="px-3">Docs</Eyebrow>
          <NavList current={current} />
        </div>
      </nav>
      <nav aria-label="Docs" className="lg:hidden">
        <details className="group rounded-xl border bg-card">
          <summary
            className={cn(
              "flex cursor-pointer list-none items-center justify-between gap-3 rounded-xl px-4 py-3 text-sm [&::-webkit-details-marker]:hidden",
              focusRing,
            )}
          >
            <span>
              <span className="text-muted-foreground">
                Docs (
                {[index, ...docsPages].findIndex(
                  page => page.slug === current,
                ) + 1}
                /{docsPages.length + 1}):{" "}
              </span>
              <span className="font-medium text-foreground">{title}</span>
            </span>
            <ChevronDown
              aria-hidden
              className="size-4 text-muted-foreground transition-transform duration-150 group-open:rotate-180"
            />
          </summary>
          <div className="border-t px-1 py-2">
            <NavList current={current} />
          </div>
        </details>
      </nav>
    </>
  );
}

function PrevNext({
  prev,
  next,
}: {
  prev?: Pick<Page, "slug" | "title">;
  next?: Page;
}) {
  if (!prev && !next) return null;
  const card = cn(
    "flex flex-col gap-1 rounded-xl border bg-card px-4 py-3 transition-colors duration-150 hover:border-brand-border",
    focusRing,
  );
  return (
    <nav
      aria-label="Previous and next page"
      className="grid gap-3 border-t pt-8 sm:grid-cols-2"
    >
      {prev ? (
        <a href={href(prev)} className={card}>
          <span className="inline-flex items-center gap-1 text-xs text-muted-foreground">
            <ArrowLeft aria-hidden className="size-3.5" /> Previous
          </span>
          <span className="font-medium text-foreground">{prev.title}</span>
        </a>
      ) : (
        <span aria-hidden className="hidden sm:block" />
      )}
      {next ? (
        <a href={href(next)} className={cn(card, "sm:items-end sm:text-right")}>
          <span className="inline-flex items-center gap-1 text-xs text-muted-foreground">
            Next <ArrowRight aria-hidden className="size-3.5" />
          </span>
          <span className="font-medium text-foreground">{next.title}</span>
        </a>
      ) : null}
    </nav>
  );
}

/** A Docs page: the Docs list, the page's heading and lead, its sections,
 * and links to the pages before and after it. `slug` "" is the overview. */
export function DocsPage({
  slug,
  title,
  lead,
  aside,
  showPagination = true,
  children,
}: {
  slug: string;
  title: string;
  lead: React.ReactNode;
  /** Under the lead: a status badge, for example. */
  aside?: React.ReactNode;
  showPagination?: boolean;
  children: React.ReactNode;
}) {
  const at = docsPages.findIndex(page => page.slug === slug);
  const prev = at === 0 ? index : at > 0 ? docsPages[at - 1] : undefined;
  const next = docsPages[at + 1];
  return (
    <Container className="grid gap-8 py-10 lg:grid-cols-[13rem_minmax(0,1fr)] lg:gap-16 lg:py-16">
      <DocsNav current={slug} />
      <article className="flex max-w-prose min-w-0 flex-col gap-12">
        <header className="flex flex-col gap-4">
          <h1 className="text-title text-balance text-foreground">{title}</h1>
          {lead ? (
            <div className="text-lead text-pretty text-muted-foreground">
              {lead}
            </div>
          ) : null}
          {aside}
        </header>
        {children}
        {showPagination ? <PrevNext prev={prev} next={next} /> : null}
      </article>
    </Container>
  );
}
