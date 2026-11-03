/** The page's frame once the node runs: the design system's AppShell with
 * the web node's pages, its sync state in the header, and a banner when the
 * node stops answering. */

import * as React from "react";
import {
  ArrowUpRight,
  Blocks,
  BookOpen,
  Box,
  Cpu,
  KeyRound,
  Layers,
  LayoutDashboard,
  ListChecks,
  Network,
  Send,
  Wallet,
} from "lucide-react";

import { Badge } from "@minanode/design-system/components/badge";
import {
  AppShell,
  SidebarNav,
  SidebarNavItem,
} from "@minanode/design-system/patterns/app-shell";
import { Banner } from "@minanode/design-system/patterns/banner";
import { Logo, LogoMark } from "@minanode/design-system/patterns/logo";
import { Signal } from "@minanode/design-system/patterns/signal";
import { ThemeToggle } from "@minanode/design-system/patterns/theme-toggle";
import { formatAge, useNow } from "@minanode/design-system/lib/time";
import {
  formatHeight,
  type NodeStatus,
  type SyncView,
  type Tone,
} from "@minanode/node-client";
import { useNodeStatus, useSyncView } from "@minanode/node-client/react";

import { CONFIG } from "../config";
import { VERSION } from "../version";
import { useStatusOnNavigate } from "../lib/tip";
import { rolesAllowed } from "../roles/role";
import { useBoot } from "./boot-context";
import { CarriedKey } from "./carried-key";
import { Link, type Route } from "./router";
import { SecondTabBanner } from "./second-tab-banner";

/** The minanode.org docs. */
const DOCS_HOME = "https://minanode.org/docs/";

type NavItem = {
  to: string;
  label: string;
  icon: React.ComponentType<{ className?: string; "aria-hidden"?: boolean }>;
  page: Route["page"];
  /** Other pages this item is current on. */
  also?: Route["page"][];
};

export const NAV: NavItem[] = [
  { to: "/", label: "Overview", icon: LayoutDashboard, page: "overview" },
  {
    to: "/blocks",
    label: "Blocks",
    icon: Blocks,
    page: "blocks",
    also: ["block"],
  },
  { to: "/accounts", label: "Accounts", icon: Wallet, page: "accounts" },
  { to: "/pool", label: "Transaction pool", icon: Layers, page: "pool" },
  { to: "/peers", label: "Peers", icon: Network, page: "peers" },
  { to: "/start", label: "Start-up", icon: ListChecks, page: "boot" },
];

/** Devnet only: the key, payments and the two roles. */
export const ROLE_NAV: NavItem[] = [
  { to: "/key", label: "Your key", icon: KeyRound, page: "key" },
  { to: "/send", label: "Send", icon: Send, page: "send" },
  { to: "/snark-worker", label: "SNARK worker", icon: Cpu, page: "worker" },
  {
    to: "/block-producer",
    label: "Block producer",
    icon: Box,
    page: "producer",
  },
];

const focusRing =
  "focus-visible:outline-2 focus-visible:outline-solid focus-visible:outline-offset-2 focus-visible:outline-ring";

/** What the header chip says. A view whose last answer is older than
 * STATUS_STALE_MS is no longer the node's status: the chip says so, with
 * the tip it had then. */
export function chipView(
  view: SyncView,
  status: NodeStatus | null,
): { tone: Tone; label: string; height: string; note: string | null } {
  const tip = status?.bestTip ?? null;
  if (view.stale && view.key !== "NoAnswer") {
    return {
      tone: "warn",
      label: "Stale",
      height: tip ? formatHeight(tip.height) : "",
      note:
        view.sampleAgeMs !== null
          ? `as of ${formatAge(view.sampleAgeMs)} ago`
          : null,
    };
  }
  const height =
    view.key === "Bootstrap" || view.key === "Catchup"
      ? status?.target
        ? `to ${formatHeight(status.target.height)}`
        : ""
      : tip
        ? formatHeight(tip.height)
        : "";
  return {
    tone: view.tone,
    label: view.label,
    height,
    note:
      view.key === "Synced" && view.tipAgeMs !== null
        ? `block ${formatAge(view.tipAgeMs)} old`
        : null,
  };
}

export function StatusChip() {
  const now = useNow();
  const view = useSyncView(now);
  const { status } = useNodeStatus();
  const chip = chipView(view, status.data);
  return (
    <Link
      to="/"
      className={`flex min-w-0 items-center gap-2 rounded-full ${focusRing}`}
      title="Node status: go to the overview"
      data-status-chip={view.stale ? "stale" : view.key}
    >
      <Signal tone={chip.tone} pulse={chip.tone === "info"}>
        {chip.label}
        {chip.height ? (
          <span className="text-foreground/80">{chip.height}</span>
        ) : null}
      </Signal>
      {chip.note ? (
        <span className="hidden font-mono text-xs text-muted-foreground sm:inline">
          {chip.note}
        </span>
      ) : null}
    </Link>
  );
}

function NodeFooter() {
  const { status, build } = useNodeStatus();
  const network = status.data?.network;
  const version = build.data?.version ?? VERSION;
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
      <ThemeToggle className="ml-auto shrink-0" />
    </div>
  );
}

function OfflineBanner() {
  const now = useNow();
  const view = useSyncView(now);
  const { status } = useNodeStatus();
  if (view.key !== "NoAnswer") return null;
  return (
    <Banner title="The web node isn't answering.">
      {status.updatedAt !== null
        ? `Figures on this page are from ${formatAge(now - status.updatedAt)} ago.`
        : "Nothing has loaded yet."}{" "}
      Its thread may be busy, or it stopped. Reloading the page starts it again.
    </Banner>
  );
}

function roleOf(page: Route["page"]) {
  return page === "worker" ? "worker" : page === "producer" ? "producer" : null;
}

export function Shell({
  route,
  children,
}: {
  route: Route;
  children: React.ReactNode;
}) {
  const role = useBoot().role.kind;
  useStatusOnNavigate();
  return (
    <AppShell
      brand={
        <Link to="/" className={`w-fit rounded-md ${focusRing}`}>
          <Logo />
        </Link>
      }
      brandCaption="Web node"
      mobileBrand={
        <Link
          to="/"
          className={`rounded-md ${focusRing}`}
          aria-label="minanode web node, overview"
        >
          <LogoMark className="size-7" />
        </Link>
      }
      navLabel="Web node"
      nav={
        <SidebarNav>
          {[...NAV, ...(rolesAllowed(CONFIG.network) ? ROLE_NAV : [])].map(
            ({ to, label, icon: Icon, page, also }) => (
              <SidebarNavItem
                key={to}
                asChild
                current={route.page === page || !!also?.includes(route.page)}
              >
                <Link to={to}>
                  <Icon aria-hidden />
                  {label}
                  {roleOf(page) === role ? (
                    <span
                      className="ml-auto size-2 rounded-full bg-status-live"
                      aria-label="on in this tab"
                    />
                  ) : null}
                </Link>
              </SidebarNavItem>
            ),
          )}
          <SidebarNavItem
            href={DOCS_HOME}
            target="_blank"
            rel="noopener noreferrer"
          >
            <BookOpen aria-hidden />
            Docs
            <ArrowUpRight aria-hidden className="ml-auto size-3.5" />
            <span className="sr-only"> (opens in a new tab)</span>
          </SidebarNavItem>
        </SidebarNav>
      }
      sidebarFooter={<NodeFooter />}
      headerEnd={<StatusChip />}
      mobileHeaderEnd={<ThemeToggle />}
      banner={
        <>
          <OfflineBanner />
          <SecondTabBanner />
        </>
      }
    >
      <CarriedKey />
      {children}
    </AppShell>
  );
}
