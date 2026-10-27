import { ArrowRight } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { MetricCard } from "@minanode/design-system/patterns/metric-card";
import { StatusBadge } from "@minanode/design-system/patterns/status-badge";

import { Container } from "@/components/container";
import { hero, proofPoints } from "@/content/site";

export function Hero() {
  return (
    <section aria-labelledby="hero-title" className="relative overflow-hidden">
      <div aria-hidden className="absolute inset-0 bg-hero-glow" />
      <Container className="relative flex flex-col items-center gap-5 pt-8 pb-12 text-center md:pt-10 md:pb-14">
        <StatusBadge status="devnet" data-claim={hero.status.claim}>
          {hero.status.label}
        </StatusBadge>

        <h1
          id="hero-title"
          data-claim={hero.title.claim}
          className="max-w-5xl text-display text-balance text-foreground"
        >
          <span className="block">{hero.title.lead}</span>{" "}
          {hero.title.accent.map((line, i) => (
            <span key={line} className="block text-gradient-brand">
              {line}
              {i < hero.title.accent.length - 1 ? " " : null}
            </span>
          ))}
        </h1>

        <p
          data-claim={hero.lead.claim}
          className="max-w-2xl text-lead text-pretty text-muted-foreground"
        >
          {hero.lead.text}
        </p>

        <div className="flex w-full flex-col items-stretch justify-center gap-3 sm:w-auto sm:flex-row sm:items-center">
          <Button size="lg" asChild>
            <a href="/docs/quickstart/">
              Download and run
              <ArrowRight aria-hidden />
            </a>
          </Button>
          <Button size="lg" variant="outline" asChild>
            <a href="#roles">Node roles</a>
          </Button>
        </div>
      </Container>

      <Container className="relative pb-10 md:pb-12">
        <h2 className="sr-only">Proof points</h2>
        <ul className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          {proofPoints.map(metric => (
            <li key={metric.claim} data-claim={metric.claim} className="flex">
              <MetricCard
                className="w-full"
                value={metric.value}
                label={metric.label}
                detail={metric.detail}
                footer={
                  metric.href ? (
                    <ExternalLink href={metric.href}>
                      {metric.hrefLabel}
                    </ExternalLink>
                  ) : undefined
                }
              />
            </li>
          ))}
        </ul>
      </Container>
    </section>
  );
}
