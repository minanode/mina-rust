import { FileQuestion } from "lucide-react";

import { EmptyState } from "@minanode/design-system/patterns/empty-state";
import { PageHeader } from "@minanode/design-system/patterns/page-header";
import { TextLink } from "@minanode/design-system/patterns/text-link";

import { Link } from "../app/router";

export function NotFoundPage({ path }: { path: string }) {
  return (
    <>
      <PageHeader title="Not found" />
      <EmptyState
        icon={<FileQuestion />}
        title="No such page"
        description={<p>This page has nothing at {path}.</p>}
        action={
          <TextLink asChild arrow>
            <Link to="/">Overview</Link>
          </TextLink>
        }
      />
    </>
  );
}
