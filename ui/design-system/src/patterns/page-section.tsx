import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

/** A titled part of an app page: an h2, a line of description, controls on
 * the right, and its content. Named by its heading for assistive tech. */
function PageSection({
  title,
  description,
  aside,
  children,
  className,
}: {
  title: React.ReactNode;
  description?: React.ReactNode;
  aside?: React.ReactNode;
  children: React.ReactNode;
  className?: string;
}) {
  const id = React.useId();
  return (
    <section
      data-slot="page-section"
      aria-labelledby={id}
      className={cn("flex flex-col gap-4", className)}
    >
      <div className="flex flex-wrap items-end justify-between gap-2">
        {/* The text takes the row and wraps, keeping the controls beside
            it until the row is too narrow for both. */}
        <div className="flex min-w-0 flex-[1_1_20rem] flex-col gap-1">
          <h2 id={id} className="text-heading text-foreground">
            {title}
          </h2>
          {description ? (
            <p className="max-w-3xl text-sm text-pretty text-muted-foreground">
              {description}
            </p>
          ) : null}
        </div>
        {aside ? <div className="shrink-0">{aside}</div> : null}
      </div>
      {children}
    </section>
  );
}

export { PageSection };
