import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

/** A placeholder block while data loads. Decorative: say "Loading" in text
 * nearby (or with aria-busy on the region) for screen readers. */
function Skeleton({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="skeleton"
      aria-hidden
      className={cn("rounded-md bg-muted motion-safe:animate-pulse", className)}
      {...props}
    />
  );
}

export { Skeleton };
