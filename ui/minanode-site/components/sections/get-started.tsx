import { SectionHeading } from "@minanode/design-system/patterns/section-heading";
import { StatusBadge } from "@minanode/design-system/patterns/status-badge";
import { Button } from "@minanode/design-system/components/button";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";

import { Container } from "@/components/container";
import { DocLink } from "@/components/docs/prose";
import { getStarted } from "@/content/site";

export function GetStarted() {
  return (
    <section id="get-started" aria-labelledby="get-started-title">
      <Container className="flex flex-col gap-8 py-10 md:py-12">
        <SectionHeading
          id="get-started-title"
          eyebrow="Get started"
          title={getStarted.title}
          description={
            <span data-claim={getStarted.claim}>{getStarted.lead}</span>
          }
        />
        <ul className="grid gap-4 md:grid-cols-2 lg:grid-cols-4">
          {getStarted.items.map(item => (
            <li
              key={item.title}
              data-claim={item.claim}
              className="flex flex-col gap-3 rounded-xl border bg-card p-6 shadow-card"
            >
              <StatusBadge status={item.status}>{item.statusLabel}</StatusBadge>
              <h3 className="text-heading text-foreground">{item.title}</h3>
              <p className="text-sm leading-relaxed text-muted-foreground">
                {item.body}
              </p>
              {item.links ? (
                <p className="mt-auto flex flex-wrap gap-x-4 gap-y-1 text-sm">
                  {item.links.map(link =>
                    link.href.startsWith("https://") ? (
                      <ExternalLink key={link.href} href={link.href}>
                        {link.label}
                      </ExternalLink>
                    ) : (
                      <DocLink key={link.href} href={link.href}>
                        {link.label}
                      </DocLink>
                    ),
                  )}
                </p>
              ) : null}
            </li>
          ))}
        </ul>
        <div>
          <Button asChild size="lg">
            <a href="/docs/quickstart/">Download and run</a>
          </Button>
        </div>
      </Container>
    </section>
  );
}
