import * as React from "react";

import { Card } from "@minanode/design-system/components/card";

/**
 * Something an app does, as a card: an icon tile, a title, its state on the
 * right (a Signal), a few lines about it, and a link to its page.
 */
function RoleCard({
  icon,
  title,
  status,
  children,
  action,
}: {
  icon: React.ReactNode;
  title: React.ReactNode;
  /** Its state: usually a Signal, or a Skeleton while loading. */
  status?: React.ReactNode;
  children: React.ReactNode;
  /** A TextLink with an arrow to the role's page. */
  action?: React.ReactNode;
}) {
  return (
    <Card data-slot="role-card" className="gap-4 px-6 py-5">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <span
            aria-hidden
            className="inline-flex size-9 items-center justify-center rounded-lg border bg-background text-brand-text [&_svg]:size-4.5"
          >
            {icon}
          </span>
          <h3 className="font-semibold text-foreground">{title}</h3>
        </div>
        {status}
      </div>
      <div className="flex flex-col gap-1 text-sm text-muted-foreground">
        {children}
      </div>
      {action}
    </Card>
  );
}

export { RoleCard };
