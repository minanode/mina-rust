"use client";

import * as React from "react";
import { Menu, X } from "lucide-react";
import { Slot } from "radix-ui";

import { Button } from "@minanode/design-system/components/button";
import { cn } from "@minanode/design-system/lib/utils";

/**
 * The frame of an app: a skip link, a sidebar with the brand, navigation and
 * a footer from 1024 px, and below that a header with a menu button that
 * opens the same navigation (a disclosure: focus goes to its first link,
 * Escape closes it and returns focus). The header's end holds app status;
 * a banner sits under it; the page goes in `main`.
 *
 * Routing is the app's: put its links in SidebarNavItem with `asChild`.
 */

type ShellContext = { closeMenu: () => void };
const Context = React.createContext<ShellContext | null>(null);

/** The shell around a component, if any: SidebarNavItem uses it to close
 * the mobile menu on navigation. */
function useAppShell(): ShellContext | null {
  return React.useContext(Context);
}

function AppShell({
  brand,
  brandCaption,
  mobileBrand,
  navLabel,
  nav,
  sidebarFooter,
  headerEnd,
  mobileHeaderEnd,
  banner,
  children,
}: {
  /** The sidebar's top: the app's logo, usually a link home. */
  brand: React.ReactNode;
  /** A mono caption under it: "Node dashboard". */
  brandCaption?: React.ReactNode;
  /** The logo in the mobile header. */
  mobileBrand?: React.ReactNode;
  /** The navigation's accessible name. */
  navLabel: string;
  /** A SidebarNav. Rendered in the sidebar and in the mobile menu. */
  nav: React.ReactNode;
  sidebarFooter?: React.ReactNode;
  /** The header's end, at every width: app status. */
  headerEnd?: React.ReactNode;
  /** More of the header's end below 1024 px, where the sidebar is hidden. */
  mobileHeaderEnd?: React.ReactNode;
  /** Under the header: a Banner when every page must say something. */
  banner?: React.ReactNode;
  children: React.ReactNode;
}) {
  const [menuOpen, setMenuOpen] = React.useState(false);
  const menuId = React.useId();
  const menuButton = React.useRef<HTMLButtonElement>(null);
  const menu = React.useRef<HTMLElement>(null);
  const context = React.useMemo(
    () => ({ closeMenu: () => setMenuOpen(false) }),
    [],
  );

  React.useEffect(() => {
    if (!menuOpen) return;
    // The menu opens below the header: take focus to its first link.
    menu.current?.querySelector<HTMLElement>("a")?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setMenuOpen(false);
        menuButton.current?.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [menuOpen]);

  return (
    <Context.Provider value={context}>
      <div data-slot="app-shell" className="min-h-dvh bg-background">
        <a
          href="#main"
          className="sr-only z-50 rounded-md bg-primary px-4 py-2 font-medium text-primary-foreground focus:not-sr-only focus:fixed focus:top-3 focus:left-3"
        >
          Skip to content
        </a>
        <div className="lg:grid lg:grid-cols-[15.5rem_minmax(0,1fr)]">
          <div className="hidden border-r bg-card/40 lg:block">
            <aside className="sticky top-0 flex h-dvh flex-col">
              <div className="flex flex-col gap-1 px-5 pt-6 pb-5">
                {brand}
                {brandCaption ? (
                  <p className="pl-[2.6rem] font-mono text-label text-muted-foreground uppercase">
                    {brandCaption}
                  </p>
                ) : null}
              </div>
              <nav
                aria-label={navLabel}
                className="flex-1 overflow-y-auto px-3"
              >
                {nav}
              </nav>
              {sidebarFooter ? (
                <div className="border-t px-5 py-4">{sidebarFooter}</div>
              ) : null}
            </aside>
          </div>

          <div className="flex min-w-0 flex-col">
            <header className="sticky top-0 z-30 border-b bg-background/90 backdrop-blur">
              <div className="flex h-14 items-center gap-3 px-gutter">
                <Button
                  ref={menuButton}
                  type="button"
                  variant="ghost"
                  size="icon-sm"
                  className="lg:hidden"
                  aria-expanded={menuOpen}
                  aria-controls={menuId}
                  aria-label={menuOpen ? "Close menu" : "Open menu"}
                  onClick={() => setMenuOpen(open => !open)}
                >
                  {menuOpen ? <X aria-hidden /> : <Menu aria-hidden />}
                </Button>
                {mobileBrand ? (
                  <div className="flex lg:hidden">{mobileBrand}</div>
                ) : null}
                <div className="ml-auto flex min-w-0 items-center gap-2">
                  {headerEnd}
                  {mobileHeaderEnd ? (
                    <div className="flex lg:hidden">{mobileHeaderEnd}</div>
                  ) : null}
                </div>
              </div>
              {menuOpen ? (
                <nav
                  ref={menu}
                  id={menuId}
                  aria-label={navLabel}
                  className="border-t bg-background px-gutter py-3 lg:hidden"
                >
                  {nav}
                  {sidebarFooter ? (
                    <div className="mt-3 border-t pt-3">{sidebarFooter}</div>
                  ) : null}
                </nav>
              ) : null}
            </header>
            {banner}
            <main
              id="main"
              tabIndex={-1}
              className="mx-auto flex w-full max-w-page flex-col gap-10 px-gutter pt-8 pb-16 outline-hidden"
            >
              {children}
            </main>
          </div>
        </div>
      </div>
    </Context.Provider>
  );
}

/** The list of an AppShell's navigation. */
function SidebarNav({ className, ...props }: React.ComponentProps<"ul">) {
  return (
    <ul
      data-slot="sidebar-nav"
      className={cn("flex flex-col gap-1", className)}
      {...props}
    />
  );
}

const navItem =
  "flex h-10 items-center gap-3 rounded-md border border-transparent px-3 text-sm font-medium text-muted-foreground transition-[color,background-color,border-color] duration-150 ease-standard hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring [&_svg]:size-4 [&_svg]:shrink-0";
const navItemCurrent =
  "border-brand-border bg-brand-subtle text-brand-text hover:bg-brand-subtle hover:text-brand-text";

/**
 * A navigation item: an icon and a label. `current` marks the page you are
 * on (aria-current="page" and the brand fill). With `asChild`, the app's
 * router link gets the styling; a click closes the mobile menu.
 */
function SidebarNavItem({
  current = false,
  asChild = false,
  className,
  onClick,
  ...props
}: React.ComponentProps<"a"> & { current?: boolean; asChild?: boolean }) {
  const shell = useAppShell();
  const Comp = asChild ? Slot.Root : "a";
  return (
    <li data-slot="sidebar-nav-item">
      <Comp
        aria-current={current ? "page" : undefined}
        className={cn(navItem, current && navItemCurrent, className)}
        onClick={(event: React.MouseEvent<HTMLAnchorElement>) => {
          onClick?.(event);
          shell?.closeMenu();
        }}
        {...props}
      />
    </li>
  );
}

export { AppShell, SidebarNav, SidebarNavItem, useAppShell };
