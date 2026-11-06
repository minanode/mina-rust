import * as React from "react";

import { Eyebrow } from "@minanode/design-system/patterns/eyebrow";

/**
 * The top of an app page: eyebrow, the page's h1 with controls on its row
 * (a data age, buttons), and a one-line lead. The h1 takes focus after an
 * in-app navigation (`titleId`, `tabIndex={-1}`), so a screen reader
 * announces the new page.
 */
function PageHeader({
  eyebrow,
  title,
  description,
  aside,
  titleId = "page-title",
}: {
  eyebrow?: React.ReactNode;
  title: React.ReactNode;
  description?: React.ReactNode;
  aside?: React.ReactNode;
  titleId?: string;
}) {
  return (
    <div data-slot="page-header" className="flex flex-col gap-1.5">
      {/* A fixed line: a back link in it doesn't move the title. */}
      {eyebrow ? (
        <Eyebrow className="flex h-4 items-center">{eyebrow}</Eyebrow>
      ) : null}
      <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-2">
        <h1
          id={titleId}
          tabIndex={-1}
          className="min-w-0 text-page text-balance text-foreground outline-hidden"
        >
          {title}
        </h1>
        {aside ? (
          // ml-auto keeps the controls on the right when they wrap. On a
          // phone a row of three or more (a role's status, the data age,
          // the docs link) takes its own line, spread from the gutter to
          // the edge, so the status starts where the title does.
          <div className="ml-auto flex flex-wrap items-center gap-x-4 gap-y-2 max-sm:[&:has(>:nth-child(3))]:ml-0 max-sm:[&:has(>:nth-child(3))]:w-full max-sm:[&:has(>:nth-child(3))]:justify-between">
            {aside}
          </div>
        ) : null}
      </div>
      {description ? (
        <p className="max-w-prose text-sm text-pretty text-muted-foreground">
          {description}
        </p>
      ) : null}
    </div>
  );
}

export { PageHeader };
