import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

/** A figure with its unit in mono, unit muted: `0.004 MINA`. A zero is
 * muted, as in the tables; a null value shows a dash. Format the figure
 * first; this only lays it out. */
function Amount({
  value,
  unit,
  className,
  unitClassName,
}: {
  value: React.ReactNode | null;
  unit: string;
  className?: string;
  /** For a large figure: a unit smaller and lighter than it. */
  unitClassName?: string;
}) {
  if (value === null) return <span className="text-muted-foreground">–</span>;
  const zero = typeof value === "string" && /^[0.,]+$/.test(value);
  return (
    <span
      data-slot="amount"
      className={cn(
        "font-mono tabular-nums whitespace-nowrap",
        zero && "text-muted-foreground",
        className,
      )}
    >
      {value}
      {/* A real space for the text, drawn as the narrow gap. */}
      <span className="text-[0px] leading-none"> </span>
      <span className={cn("ml-1 text-muted-foreground", unitClassName)}>
        {unit}
      </span>
    </span>
  );
}

/** A measured figure with its units, such as `338 ms`, `1 min 48 s` or
 * `76.9 MB`, set as Amount sets MINA: figures in mono, units muted, with a
 * narrow gap where a mono space would be a full character wide. Give it the
 * formatted text; its words are taken as units. A null value shows a dash. */
function Measure({
  value,
  className,
  unitClassName,
}: {
  value: string | null;
  className?: string;
  /** For a large figure: a unit smaller and lighter than it. */
  unitClassName?: string;
}) {
  if (value === null) return <span className="text-muted-foreground">–</span>;
  const parts = value.split(/\s+/).filter(Boolean);
  return (
    <span
      data-slot="measure"
      className={cn("font-mono tabular-nums whitespace-nowrap", className)}
    >
      {parts.map((part, i) => (
        <React.Fragment key={i}>
          {/* A real space for the text (a reader, a copy), drawn as the
              narrow gap below rather than a full mono space. */}
          {i > 0 ? <span className="text-[0px] leading-none"> </span> : null}
          {/^[A-Za-zµ]+$/.test(part) ? (
            <span className={cn("ml-1 text-muted-foreground", unitClassName)}>
              {part}
            </span>
          ) : (
            <span className={i > 0 ? "ml-1.5" : undefined}>{part}</span>
          )}
        </React.Fragment>
      ))}
    </span>
  );
}

export { Amount, Measure };
