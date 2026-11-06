import * as React from "react";
import { TriangleAlert } from "lucide-react";

import { cn } from "@minanode/design-system/lib/utils";

/**
 * A full-width line under an app's header for something every page must
 * know (a source that stopped answering). Announced once (role="alert").
 */
function Banner({
  title,
  children,
  action,
  tone = "destructive",
  className,
}: {
  title: React.ReactNode;
  children?: React.ReactNode;
  /** The way out (a reload), at the line's end; under it on a phone. */
  action?: React.ReactNode;
  tone?: "destructive" | "caution";
  className?: string;
}) {
  return (
    <div
      data-slot="banner"
      role="alert"
      className={cn(
        "border-b px-gutter py-3 text-sm text-foreground",
        tone === "destructive"
          ? "border-destructive/40 bg-destructive/8"
          : "border-status-pending/40 bg-caution-wash",
        className,
      )}
    >
      <div className="mx-auto flex max-w-page flex-wrap items-center gap-x-3 gap-y-1">
        {/* The icon every caution and failure carries. */}
        <TriangleAlert
          aria-hidden
          className={cn(
            "size-4 shrink-0",
            tone === "destructive" ? "text-destructive" : "text-status-pending",
          )}
        />
        <span className="font-medium">{title}</span>
        {children ? (
          <span className="text-muted-foreground">{children}</span>
        ) : null}
        {/* On a phone under the text, past the icon (16 px and its 12 px
            gap), as an alert's action sits. */}
        {action ? (
          <span className="max-sm:basis-full max-sm:pl-7 sm:ml-auto">
            {action}
          </span>
        ) : null}
      </div>
    </div>
  );
}

export { Banner };
