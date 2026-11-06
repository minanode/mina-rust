/** Table and panel figures, set the same way on every page. */

import { cn } from "@minanode/design-system/lib/utils";
import { formatCount, formatMina } from "@minanode/node-client";

/** A height or hash that opens its page: in the text colour, as a column
 * of them would be a column of teal otherwise; teal and underlined on
 * hover. */
export const TABLE_LINK =
  "rounded-sm text-foreground underline decoration-border underline-offset-4 hover:text-brand-text hover:decoration-current focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring";

/** The same, for an ExternalLink (the explorer): its own colours and
 * weight overridden. */
export const QUIET_EXTERNAL_LINK =
  "font-normal text-foreground decoration-border hover:text-brand-text";

/** A count, muted at zero. */
export function Count({ n }: { n: number | null }) {
  return (
    <span className={n === 0 ? "text-muted-foreground" : undefined}>
      {formatCount(n)}
    </span>
  );
}

/** MINA in a column whose head names the unit: the figure alone, muted at
 * zero. */
export function Mina({ nanomina }: { nanomina: number | bigint | null }) {
  if (nanomina === null) {
    return <span className="text-muted-foreground">–</span>;
  }
  const zero = nanomina === 0 || nanomina === 0n;
  return (
    <span
      className={cn("font-mono tabular-nums", zero && "text-muted-foreground")}
    >
      {formatMina(nanomina)}
    </span>
  );
}
