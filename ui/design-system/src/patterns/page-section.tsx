import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";

/** A titled part of an app page: an h2, a line of description, controls on
 * the right, and its content. Named by its heading for assistive tech. */
function PageSection({
  title,
  description,
  aside,
  stackOnPhone = false,
  children,
  className,
}: {
  title: React.ReactNode;
  description?: React.ReactNode;
  aside?: React.ReactNode;
  /** Below 640 px the controls take their own row under the title, for a
   * row of several that would crowd it. */
  stackOnPhone?: boolean;
  children: React.ReactNode;
  className?: string;
}) {
  const id = React.useId();
  return (
    <section
      data-slot="page-section"
      aria-labelledby={id}
      // Room above a section's head, and its head close to its content.
      className={cn("flex flex-col gap-3 pt-2", className)}
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        {/* The text takes the row and wraps, keeping the controls beside
            it until the row is too narrow for both. */}
        <div className="flex min-w-0 flex-[1_1_6rem] flex-col gap-1">
          <h2 id={id} className="text-heading text-foreground">
            {title}
          </h2>
          {description ? (
            <p className="max-w-3xl text-sm text-pretty text-muted-foreground">
              {description}
            </p>
          ) : null}
        </div>
        {/* ml-auto keeps the controls on the right when they wrap. */}
        {aside ? (
          <div
            className={cn(
              "ml-auto shrink-0",
              stackOnPhone && "max-sm:basis-full",
            )}
          >
            {aside}
          </div>
        ) : null}
      </div>
      {children}
    </section>
  );
}

export { PageSection };
