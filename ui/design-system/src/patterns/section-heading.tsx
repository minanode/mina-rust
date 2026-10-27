import * as React from "react";

import { cn } from "@minanode/design-system/lib/utils";
import { Eyebrow } from "@minanode/design-system/patterns/eyebrow";

function SectionHeading({
  eyebrow,
  title,
  description,
  as: Heading = "h2",
  id,
  align = "start",
  className,
}: {
  eyebrow?: React.ReactNode;
  title: React.ReactNode;
  description?: React.ReactNode;
  as?: "h1" | "h2" | "h3";
  /** id for the heading, for aria-labelledby on the section. */
  id?: string;
  align?: "start" | "center";
  className?: string;
}) {
  return (
    <div
      data-slot="section-heading"
      className={cn(
        "flex max-w-prose flex-col gap-3",
        align === "center" && "mx-auto items-center text-center",
        className,
      )}
    >
      {eyebrow ? <Eyebrow>{eyebrow}</Eyebrow> : null}
      <Heading id={id} className="text-title text-balance text-foreground">
        {title}
      </Heading>
      {description ? (
        <p className="text-lead text-pretty text-muted-foreground">
          {description}
        </p>
      ) : null}
    </div>
  );
}

export { SectionHeading };
