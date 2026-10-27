import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

/** Small monospace label above a heading or inside a panel. */
function Eyebrow({ className, ...props }: React.ComponentProps<"p">) {
  return (
    <p
      data-slot="eyebrow"
      className={cn(
        "font-mono text-label text-brand-text uppercase",
        className,
      )}
      {...props}
    />
  );
}

export { Eyebrow };
