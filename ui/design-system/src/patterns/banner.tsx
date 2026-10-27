import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

/**
 * A full-width line under an app's header for something every page must
 * know (a source that stopped answering). Announced once (role="alert").
 */
function Banner({
  title,
  children,
  tone = "destructive",
  className,
}: {
  title: React.ReactNode;
  children?: React.ReactNode;
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
          : "border-status-pending/40 bg-status-pending/8",
        className,
      )}
    >
      <div className="mx-auto flex max-w-page flex-wrap items-center gap-x-3 gap-y-1">
        <span className="font-medium">{title}</span>
        {children ? (
          <span className="text-muted-foreground">{children}</span>
        ) : null}
      </div>
    </div>
  );
}

export { Banner };
