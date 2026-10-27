import * as React from "react";

import { Eyebrow } from "@minanode/design-system/patterns/eyebrow";

/**
 * The top of an app page: eyebrow, the page's h1, a lead, and controls on
 * the right (a data age, buttons). The h1 takes focus after an in-app
 * navigation (`titleId`, `tabIndex={-1}`), so a screen reader announces the
 * new page.
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
    <div
      data-slot="page-header"
      className="flex flex-col gap-4 pb-6 md:flex-row md:items-end md:justify-between"
    >
      <div className="flex max-w-prose flex-col gap-2">
        {eyebrow ? <Eyebrow>{eyebrow}</Eyebrow> : null}
        <h1
          id={titleId}
          tabIndex={-1}
          className="text-title text-balance text-foreground outline-hidden"
        >
          {title}
        </h1>
        {description ? (
          <p className="text-pretty text-muted-foreground">{description}</p>
        ) : null}
      </div>
      {aside ? (
        <div className="flex flex-wrap items-center gap-2">{aside}</div>
      ) : null}
    </div>
  );
}

export { PageHeader };
