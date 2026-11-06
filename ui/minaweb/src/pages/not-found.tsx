import { ArrowRight, FileQuestion } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { PageHeader } from "@minanode/design-system/patterns/page-header";

import { Link } from "../app/router";

export function NotFoundPage({ path }: { path: string }) {
  return (
    // The width of the other one-panel pages (Your key, Send).
    <div className="flex max-w-3xl flex-col gap-6 lg:gap-8">
      {/* An eyebrow as on every page, so the title sits on the same line. */}
      <PageHeader eyebrow="Not found" title="No such page" />
      <EmptyState
        icon={<FileQuestion />}
        title={
          <>
            Nothing at{" "}
            <span className="font-mono text-[0.9375rem] font-normal">
              {path}
            </span>
          </>
        }
        action={
          <Button asChild variant="outline">
            <Link to="/">
              Go to Overview
              <ArrowRight aria-hidden />
            </Link>
          </Button>
        }
      />
    </div>
  );
}
