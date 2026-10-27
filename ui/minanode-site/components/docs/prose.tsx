import * as React from "react";
import { TriangleAlert } from "lucide-react";

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@minanode/design-system/components/alert";
import { cn } from "@minanode/design-system/lib/utils";
import type { KeyValueRow } from "@minanode/design-system/patterns/key-value-list";
import { TextLink } from "@minanode/design-system/patterns/text-link";

/** Body copy on a Docs page: paragraphs, lists and inline code, spaced. */
export function Prose({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      className={cn(
        "flex flex-col gap-4 text-[0.9375rem] leading-relaxed text-foreground",
        "[&_h3]:mt-3 [&_h3]:first:mt-0 [&_h3]:text-base [&_h3]:font-semibold [&_h3]:tracking-tight",
        "[&_ol]:flex [&_ol]:list-decimal [&_ol]:flex-col [&_ol]:gap-2 [&_ol]:pl-5",
        "[&_ul]:flex [&_ul]:list-disc [&_ul]:flex-col [&_ul]:gap-2 [&_ul]:pl-5",
        "[&_li]:pl-1 [&_li]:marker:text-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}

/** A link to another page in running text: TextLink, underlined as
 * ExternalLink is, so it doesn't rely on colour alone. */
export function DocLink({
  className,
  ...props
}: React.ComponentProps<typeof TextLink>) {
  return (
    <TextLink
      className={cn(
        "underline decoration-brand-border hover:decoration-current",
        className,
      )}
      {...props}
    />
  );
}

/** A titled part of a Docs page, linkable by its id. */
export function DocSection({
  id,
  title,
  children,
}: {
  id: string;
  title: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section aria-labelledby={id} className="flex flex-col gap-4">
      <h2 id={id} className="scroll-mt-24 text-heading text-foreground">
        {title}
      </h2>
      {children}
    </section>
  );
}

/** Inline code: a command, flag, file or value in running text. */
export function C({ children }: { children: React.ReactNode }) {
  return (
    <code className="rounded-sm bg-muted px-1 py-0.5 font-mono text-[0.8125rem] break-words text-foreground">
      {children}
    </code>
  );
}

/** A warning that has to be read: the node's HTTP port, a key file. */
export function Caution({
  title,
  children,
}: {
  title: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <Alert variant="caution">
      <TriangleAlert aria-hidden />
      <AlertTitle>{title}</AlertTitle>
      <AlertDescription className="gap-2">{children}</AlertDescription>
    </Alert>
  );
}

/** KeyValueList rows from content rows, each value tagged with its claim. */
export function claimedRows(
  rows: { claim: string; label: string; value: string }[],
): KeyValueRow[] {
  return rows.map(row => ({
    label: row.label,
    value: <span data-claim={row.claim}>{row.value}</span>,
  }));
}

/** Terms and what they are, in the body font: the dashboard's pages, the
 * files in the work directory. KeyValueList is for short values. */
export function Terms({
  items,
  mono = false,
}: {
  items: { term: React.ReactNode; text: React.ReactNode }[];
  /** Set the terms in monospace: file names, flags. */
  mono?: boolean;
}) {
  return (
    <dl className="flex flex-col divide-y rounded-xl border bg-card">
      {items.map((item, i) => (
        <div
          key={i}
          className="flex flex-col gap-1 px-4 py-3 sm:grid sm:grid-cols-[11rem_minmax(0,1fr)] sm:gap-4"
        >
          <dt
            className={cn(
              "font-medium break-words text-foreground",
              mono
                ? "font-mono text-[0.8125rem] leading-6"
                : "text-sm leading-6",
            )}
          >
            {item.term}
          </dt>
          <dd className="text-sm leading-6 text-muted-foreground">
            {item.text}
          </dd>
        </div>
      ))}
    </dl>
  );
}
