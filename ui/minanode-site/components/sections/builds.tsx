import { Eyebrow } from "@minanode/design-system/patterns/eyebrow";
import { SectionHeading } from "@minanode/design-system/patterns/section-heading";
import { Stat } from "@minanode/design-system/patterns/stat";

import { Container } from "@/components/container";
import { builds } from "@/content/site";

export function Builds() {
  return (
    <section id="builds" aria-labelledby="builds-title">
      <Container className="flex flex-col gap-8 py-10 md:py-12">
        <SectionHeading
          id="builds-title"
          eyebrow="Native and web"
          title="One codebase, two builds"
        />
        <div className="grid gap-6 lg:grid-cols-2">
          {builds.map(build => (
            <article
              key={build.id}
              aria-labelledby={`build-${build.id}`}
              className="flex flex-col gap-6 rounded-2xl border bg-card p-6 shadow-card md:p-8"
            >
              <div className="flex flex-col gap-3">
                <Eyebrow>{build.eyebrow}</Eyebrow>
                <h3
                  id={`build-${build.id}`}
                  className="text-[1.5rem] leading-tight font-semibold tracking-tight text-foreground"
                >
                  {build.title}
                </h3>
                <p
                  data-claim={build.body.claim}
                  className="leading-relaxed text-muted-foreground"
                >
                  {build.body.text}
                </p>
              </div>
              <ul className="grid gap-6 border-t pt-6 sm:grid-cols-3">
                {build.metrics.map(metric => (
                  <li key={metric.claim} data-claim={metric.claim}>
                    <Stat
                      value={metric.value}
                      label={metric.label}
                      detail={metric.detail}
                    />
                  </li>
                ))}
              </ul>
            </article>
          ))}
        </div>
      </Container>
    </section>
  );
}
