import { Blocks, Cpu, Globe, Network } from "lucide-react";

import { ExternalLink } from "@minanode/design-system/patterns/external-link";
import { SectionHeading } from "@minanode/design-system/patterns/section-heading";
import { StatusBadge } from "@minanode/design-system/patterns/status-badge";

import { Container } from "@/components/container";
import { about, devnetBlock, roles } from "@/content/site";

const icons: Record<string, typeof Network> = {
  "full-node": Network,
  "block-producer": Blocks,
  "snark-worker": Cpu,
  "web-node": Globe,
};

export function Roles() {
  return (
    <section id="roles" aria-labelledby="roles-title">
      <Container className="flex flex-col gap-8 py-10 md:py-12">
        <SectionHeading
          id="roles-title"
          eyebrow="What it is"
          title={about.title}
          description={<span data-claim={about.claim}>{about.lead}</span>}
        />
        <ul className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          {roles.map(role => {
            const Icon = icons[role.id];
            return (
              <li
                key={role.id}
                data-claim={role.claim}
                className="flex flex-col gap-4 rounded-xl border bg-card p-6 shadow-card"
              >
                <div className="flex items-center justify-between gap-3">
                  <Icon aria-hidden className="size-5 text-brand-text" />
                  <StatusBadge status={role.status}>{role.statusLabel}</StatusBadge>
                </div>
                <h3 className="text-heading text-foreground">{role.title}</h3>
                <p className="text-sm leading-relaxed text-muted-foreground">
                  {role.text}
                </p>
                {role.link ? (
                  <ExternalLink
                    href={devnetBlock(role.link.height)}
                    className="mt-auto font-mono text-xs"
                  >
                    Block {role.link.height}
                  </ExternalLink>
                ) : null}
              </li>
            );
          })}
        </ul>
      </Container>
    </section>
  );
}
