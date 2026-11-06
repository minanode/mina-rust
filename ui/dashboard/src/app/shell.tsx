/** The dashboard's frame: the design system's AppShell with the node's
 * navigation, its status in the header, and the no-answer banner. */

import * as React from "react";
import {
  ArrowUpRight,
  Blocks,
  BookOpen,
  Box,
  Braces,
  Bug,
  Cpu,
  FileJson,
  Layers,
  LayoutDashboard,
  Network,
  Wallet,
} from "lucide-react";

import { Badge } from "@minanode/design-system/components/badge";
import {
  AppShell,
  SidebarNav,
  SidebarNavGroup,
  SidebarNavItem,
} from "@minanode/design-system/patterns/app-shell";
import { Banner } from "@minanode/design-system/patterns/banner";
import { Logo, LogoMark } from "@minanode/design-system/patterns/logo";
import { Signal } from "@minanode/design-system/patterns/signal";
import { ThemeToggle } from "@minanode/design-system/patterns/theme-toggle";
import { formatAge, useNow } from "@minanode/design-system/lib/time";
import { formatHeight } from "@minanode/node-client";
import { useNodeStatus, useSyncView } from "@minanode/node-client/react";

import { DOCS_HOME } from "../lib/docs";
import { Link, type Route } from "./router";

type NavItem = {
  to: string;
  label: string;
  icon: React.ComponentType<{ className?: string; "aria-hidden"?: boolean }>;
  pages: Route["page"][];
};

/** The sidebar's groups, each with its pages. A group's name is its pages'
 * eyebrow. */
export const NAV_GROUPS: [string, NavItem[]][] = [
  [
    "Node",
    [
      {
        to: "/",
        label: "Overview",
        icon: LayoutDashboard,
        pages: ["overview"],
      },
      { to: "/peers", label: "Peers", icon: Network, pages: ["peers"] },
      {
        to: "/node-status",
        label: "Status",
        icon: FileJson,
        pages: ["status"],
      },
      {
        to: "/graphql-endpoint",
        label: "GraphQL",
        icon: Braces,
        pages: ["graphql"],
      },
    ],
  ],
  [
    "Chain",
    [
      {
        to: "/blocks",
        label: "Blocks",
        icon: Blocks,
        pages: ["blocks", "block"],
      },
      { to: "/account", label: "Accounts", icon: Wallet, pages: ["account"] },
      { to: "/mempool", label: "Mempool", icon: Layers, pages: ["mempool"] },
    ],
  ],
  [
    "Roles",
    [
      {
        to: "/snark-worker",
        label: "SNARK worker",
        icon: Cpu,
        pages: ["worker"],
      },
      {
        to: "/block-producer",
        label: "Block producer",
        icon: Box,
        pages: ["producer"],
      },
    ],
  ],
];

const REPORT: NavItem = {
  to: "/report-a-problem",
  label: "Report a problem",
  icon: Bug,
  pages: ["report"],
};

const focusRing =
  "focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring";

/** The sync phase, the best tip and its age, in the header. */
function StatusChip() {
  const now = useNow();
  const view = useSyncView(now);
  const { status } = useNodeStatus();
  const tip = status.data?.bestTip ?? null;
  const height =
    view.key === "Bootstrap" || view.key === "Catchup"
      ? status.data?.target
        ? `to ${formatHeight(status.data.target.height)}`
        : ""
      : tip
        ? formatHeight(tip.height)
        : "";
  return (
    <Link
      to="/"
      className={`flex min-w-0 items-center gap-2 rounded-full ${focusRing}`}
      title="Node status: go to the overview"
    >
      {view.key === "Synced" && view.tipAgeMs !== null ? (
        // Before the pill, right-aligned: the pill stays on the content's
        // right edge as the age's width changes.
        <span className="hidden text-right font-sans text-xs text-muted-foreground tabular-nums sm:inline">
          block {formatAge(view.tipAgeMs)} old
        </span>
      ) : null}
      <Signal tone={view.tone} pulse={view.tone === "info"}>
        {view.label}
        {height ? (
          // "to" is a word; only the height is a figure.
          <span className="text-foreground/80">
            {height.startsWith("to ") ? (
              <>
                to <span className="font-mono">{height.slice(3)}</span>
              </>
            ) : (
              <span className="font-mono">{height}</span>
            )}
          </span>
        ) : null}
      </Signal>
    </Link>
  );
}

/** The network and the version, with the theme toggle. */
function NodeFooter() {
  const { status, build } = useNodeStatus();
  const network = status.data?.network;
  const version = build.data?.version;
  return (
    <div className="flex items-center gap-2">
      {network ? <Badge variant="default">{network}</Badge> : null}
      {version ? (
        <span
          className="min-w-0 truncate font-mono text-xs text-muted-foreground"
          title={version}
        >
          {version}
        </span>
      ) : null}
      {/* Its glyph on the Docs arrow's line. */}
      <ThemeToggle className="-mr-1 ml-auto shrink-0" />
    </div>
  );
}

/** Once /status stops answering, every page says so, with how old its
 * figures are. */
function OfflineBanner() {
  const now = useNow();
  const view = useSyncView(now);
  const { status } = useNodeStatus();
  if (view.key !== "NoAnswer") return null;
  return (
    <Banner title="The node isn't answering.">
      {status.updatedAt !== null
        ? `Figures on this page are from ${formatAge(now - status.updatedAt)} ago.`
        : "Nothing has loaded yet."}{" "}
      The dashboard keeps asking every 5 s.
    </Banner>
  );
}

function NavLink({ item, route }: { item: NavItem; route: Route }) {
  const { to, label, icon: Icon, pages } = item;
  return (
    <SidebarNavItem asChild current={pages.includes(route.page)}>
      <Link to={to}>
        <Icon aria-hidden />
        {label}
      </Link>
    </SidebarNavItem>
  );
}

export function Shell({
  route,
  children,
}: {
  route: Route;
  children: React.ReactNode;
}) {
  return (
    <AppShell
      brand={
        // The caption beside the wordmark, behind a hairline: one line in
        // the header-high brand row.
        <div className="flex items-center gap-3">
          <Link to="/" className={`w-fit shrink-0 rounded-md ${focusRing}`}>
            <Logo className="text-[20px]" markClassName="size-7" />
          </Link>
          <span className="border-l pl-3 font-mono text-[0.625rem] leading-none tracking-wider whitespace-nowrap text-muted-foreground uppercase">
            Dashboard
          </span>
        </div>
      }
      mobileBrand={
        <Link
          to="/"
          className={`rounded-md ${focusRing}`}
          aria-label="minanode node dashboard, overview"
        >
          <LogoMark className="size-7" />
        </Link>
      }
      navLabel="Dashboard"
      nav={
        <SidebarNav>
          {NAV_GROUPS.map(([group, items]) => (
            <SidebarNavGroup key={group} label={group}>
              {items.map(item => (
                <NavLink key={item.to} item={item} route={route} />
              ))}
            </SidebarNavGroup>
          ))}
          <SidebarNavGroup label="Help">
            <NavLink item={REPORT} route={route} />
            <SidebarNavItem
              href={DOCS_HOME}
              target="_blank"
              rel="noopener noreferrer"
              data-docs-link
            >
              <BookOpen aria-hidden />
              Docs
              <ArrowUpRight aria-hidden className="ml-auto size-3.5" />
              <span className="sr-only"> (opens in a new tab)</span>
            </SidebarNavItem>
          </SidebarNavGroup>
        </SidebarNav>
      }
      sidebarFooter={<NodeFooter />}
      headerEnd={<StatusChip />}
      mobileHeaderEnd={<ThemeToggle />}
      banner={<OfflineBanner />}
    >
      {children}
    </AppShell>
  );
}
