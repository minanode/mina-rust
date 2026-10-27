import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

/** A figure with its unit in mono, unit muted: `0.004 MINA`. A null value
 * shows a dash. Format the figure first; this only lays it out. */
function Amount({
  value,
  unit,
  className,
}: {
  value: React.ReactNode | null;
  unit: string;
  className?: string;
}) {
  if (value === null) return <span className="text-muted-foreground">–</span>;
  return (
    <span
      data-slot="amount"
      className={cn("font-mono tabular-nums whitespace-nowrap", className)}
    >
      {value}
      <span className="ml-1 text-muted-foreground">{unit}</span>
    </span>
  );
}

export { Amount };
