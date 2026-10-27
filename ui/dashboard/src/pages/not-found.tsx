import { Compass } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { PageHeader } from "@minanode/design-system/patterns/page-header";

import { Link } from "../app/router";

export function NotFoundPage({ path }: { path: string }) {
  return (
    <div>
      <PageHeader title="No page here" />
      <EmptyState
        icon={<Compass />}
        title={`The dashboard has no page at ${path}`}
        description={
          <p>Pick a page from the menu, or go back to the overview.</p>
        }
        action={
          <Button asChild variant="outline" size="sm">
            <Link to="/">Overview</Link>
          </Button>
        }
      />
    </div>
  );
}
