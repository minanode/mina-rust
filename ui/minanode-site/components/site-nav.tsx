"use client";

import * as React from "react";
import { usePathname } from "next/navigation";
import { Menu, X } from "lucide-react";

import { Button } from "@minanode/design-system/components/button";
import { cn } from "@minanode/design-system/lib/utils";

type Item = { href: string; label: string };

/** Site links: inline from md up, a disclosure menu below md. */
export function SiteNav({
  items,
  label = "Sections",
}: {
  items: Item[];
  label?: string;
}) {
  const pathname = usePathname();
  const active = (href: string) => {
    if (!href.startsWith("/")) return false;
    if (href === "/") return pathname === "/";
    const section = href.replace(/\/$/, "");
    return pathname === section || pathname.startsWith(`${section}/`);
  };
  const [open, setOpen] = React.useState(false);
  const menuId = React.useId();
  const buttonRef = React.useRef<HTMLButtonElement>(null);

  React.useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOpen(false);
        buttonRef.current?.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open]);

  const link =
    "rounded-md px-3 py-2 text-sm text-muted-foreground transition-colors duration-150 hover:bg-accent hover:text-foreground";

  return (
    <>
      <nav aria-label={label} className="hidden md:block">
        <ul className="flex items-center gap-1">
          {items.map(item => (
            <li key={item.href}>
              <a
                href={item.href}
                aria-current={active(item.href) ? "page" : undefined}
                className={cn(
                  link,
                  active(item.href) && "bg-accent font-medium text-foreground",
                )}
              >
                {item.label}
              </a>
            </li>
          ))}
        </ul>
      </nav>
      <div className="md:hidden">
        <Button
          ref={buttonRef}
          type="button"
          variant="ghost"
          size="icon-sm"
          className="size-10 sm:size-8"
          aria-expanded={open}
          aria-controls={menuId}
          aria-label={open ? "Close menu" : "Open menu"}
          onClick={() => setOpen(value => !value)}
        >
          {open ? <X /> : <Menu />}
        </Button>
        <nav
          id={menuId}
          aria-label={label}
          hidden={!open}
          className="absolute inset-x-0 top-full border-b bg-background shadow-lg"
        >
          <ul className="mx-auto flex max-w-page flex-col gap-1 px-gutter py-3">
            {items.map(item => (
              <li key={item.href}>
                <a
                  href={item.href}
                  aria-current={active(item.href) ? "page" : undefined}
                  className={cn(
                    link,
                    "block py-3 text-base",
                    active(item.href) &&
                      "bg-accent font-medium text-foreground",
                  )}
                  onClick={() => setOpen(false)}
                >
                  {item.label}
                </a>
              </li>
            ))}
          </ul>
        </nav>
      </div>
    </>
  );
}
