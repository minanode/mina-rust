import { Badge } from "@minanode/design-system/components/badge";
import { Logo } from "@minanode/design-system/patterns/logo";
import { ThemeToggle } from "@minanode/design-system/patterns/theme-toggle";

import { Container } from "@/components/container";
import { nav } from "@/content/site";
import { SiteNav } from "@/components/site-nav";

export function SiteHeader() {
  return (
    <header className="sticky top-0 z-40 border-b bg-background/80 backdrop-blur-md supports-[backdrop-filter]:bg-background/70">
      <Container className="relative flex h-16 items-center gap-4">
        <a
          href="/"
          className="-mx-1 rounded-md px-1"
          aria-label="minanode home"
        >
          <Logo markClassName="size-8" />
        </a>
        <Badge variant="outline" className="-ml-1 rounded-full">
          <span aria-hidden>Beta</span>
          <span className="sr-only">Beta release</span>
        </Badge>
        <div className="ml-auto flex items-center gap-1">
          <SiteNav items={nav} label="Site" />
          <ThemeToggle />
        </div>
      </Container>
    </header>
  );
}
