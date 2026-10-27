import { Logo } from "@minanode/design-system/patterns/logo";

import { Container } from "@/components/container";
import { nav } from "@/content/site";

export function SiteFooter({ homeHref = "" }: { homeHref?: string }) {
  const link =
    "inline-flex min-h-10 items-center rounded-sm py-2 text-muted-foreground transition-colors duration-150 hover:text-foreground";
  return (
    <footer className="border-t">
      <Container className="flex flex-col gap-6 py-10">
        <div className="flex flex-col gap-6 md:flex-row md:items-center md:justify-between">
          <Logo />
          <nav aria-label="Footer">
            <ul className="flex flex-wrap gap-x-6 gap-y-2 text-sm">
              {nav.map(item => (
                <li key={item.href}>
                  <a
                    href={
                      item.href.startsWith("#")
                        ? `${homeHref}${item.href}`
                        : item.href
                    }
                    className={link}
                  >
                    {item.label}
                  </a>
                </li>
              ))}
              <li>
                <a href="/licenses/" className={link}>
                  Licenses
                </a>
              </li>
            </ul>
          </nav>
        </div>
      </Container>
    </footer>
  );
}
