import * as React from "react";

/** A command, flag or file name inside a sentence. Put real spaces around
 * it in the text, not a margin: a margin shows as a stray space before a
 * full stop, and text that touches it can't wrap there. */
export function Code({ children }: { children: React.ReactNode }) {
  return (
    <code className="font-mono whitespace-nowrap text-foreground">
      {children}
    </code>
  );
}

/** What a role needs at the node's next start, as rows: a label, then the
 * flag or variable. On a phone the flag goes under its label. */
export function Flags({
  rows,
}: {
  rows: { label: React.ReactNode; flag: React.ReactNode }[];
}) {
  return (
    <dl className="mt-2 grid grid-cols-1 items-baseline gap-x-6 sm:grid-cols-[max-content_minmax(0,1fr)] sm:gap-y-1">
      {rows.map((row, i) => (
        <React.Fragment key={i}>
          <dt className={i > 0 ? "mt-2 sm:mt-0" : undefined}>{row.label}</dt>
          <dd className="font-mono text-[0.8125rem] break-words text-foreground">
            {row.flag}
          </dd>
        </React.Fragment>
      ))}
    </dl>
  );
}
