import * as React from "react";
import { ArrowUpRight } from "lucide-react";

import { cn } from "@minanode/design-system/lib/utils";

/** A link that leaves the site: opens in a new tab and says so. */
function ExternalLink({
  className,
  children,
  icon = true,
  ...props
}: React.ComponentProps<"a"> & { icon?: boolean }) {
  return (
    <a
      data-slot="external-link"
      target="_blank"
      rel="noopener noreferrer"
      className={cn(
        "inline-flex items-center gap-0.5 rounded-sm font-medium text-brand-text underline decoration-brand-border underline-offset-4 transition-colors duration-150 hover:decoration-current focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring",
        className,
      )}
      {...props}
    >
      {children}
      {icon ? <ArrowUpRight aria-hidden className="size-3.5 shrink-0" /> : null}
      <span className="sr-only"> (opens in a new tab)</span>
    </a>
  );
}

export { ExternalLink };
