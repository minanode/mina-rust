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
  RefreshCw,
  Send,
  Wallet,
} from "lucide-react";

import { Badge } from "@minanode/design-system/components/badge";
import { Button } from "@minanode/design-system/components/button";
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
import {
  formatHeight,
  type NodeStatus,
  type SyncView,
  type Tone,
} from "@minanode/node-client";
import { useNodeStatus, useSyncView } from "@minanode/node-client/react";

import { reloadAndStart } from "../boot/autostart";
import { CONFIG } from "../config";
import { VERSION } from "../version";
import { useStatusOnNavigate } from "../lib/tip";
import { RestartConfirm } from "../pages/restart-confirm";
import { useKeys } from "../roles/hooks";
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

/** The node in this tab. Its group name is its pages' eyebrow. */
export const NODE_NAV: NavItem[] = [
  { to: "/", label: "Overview", icon: LayoutDashboard, page: "overview" },
  { to: "/peers", label: "Peers", icon: Network, page: "peers" },
  { to: "/start", label: "Start-up", icon: ListChecks, page: "boot" },
];

/** What the node holds of the chain. */
export const CHAIN_NAV: NavItem[] = [
  {
    to: "/blocks",
    label: "Blocks",
    icon: Blocks,
    page: "blocks",
    also: ["block"],
  },
  { to: "/accounts", label: "Accounts", icon: Wallet, page: "accounts" },
  { to: "/pool", label: "Transaction pool", icon: Layers, page: "pool" },
];

export const NAV: NavItem[] = [...NODE_NAV, ...CHAIN_NAV];

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

/** The sidebar's groups, each with its pages. */
function navGroups(): [string, NavItem[]][] {
  return [
    ["Node", NODE_NAV],
    ["Chain", CHAIN_NAV],
    ...(rolesAllowed(CONFIG.network)
      ? [["Devnet", ROLE_NAV] as [string, NavItem[]]]
      : []),
  ];
}

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
      {chip.note ? (
        // Before the pill, right-aligned: the pill stays on the content's
        // right edge as the age's width changes.
        <span className="hidden text-right font-sans text-xs text-muted-foreground tabular-nums sm:inline">
          {chip.note}
        </span>
      ) : null}
      <Signal tone={chip.tone} pulse={chip.tone === "info"}>
        {chip.label}
        {chip.height ? (
          // "to" is a word; only the height is a figure.
          <span className="text-foreground/80">
            {chip.height.startsWith("to ") ? (
              <>
                to <span className="font-mono">{chip.height.slice(3)}</span>
              </>
            ) : (
              <span className="font-mono">{chip.height}</span>
            )}
          </span>
        ) : null}
      </Signal>
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
      {/* Its glyph on the Docs arrow's line. */}
      <ThemeToggle className="-mr-1 ml-auto shrink-0" />
    </div>
  );
}

export function OfflineBanner() {
  const now = useNow();
  const view = useSyncView(now);
  const { status } = useNodeStatus();
  const { key } = useKeys();
  const { role } = useBoot();
  const [confirming, setConfirming] = React.useState(false);
  if (view.key !== "NoAnswer") return null;
  return (
    <>
      <Banner
        title="The web node isn't answering."
        action={
          confirming ? null : (
            // Reloading starts the node again. A reload empties the tab's
            // memory, so with a key held it is a restart: its confirm
            // keeps the key and the role across it.
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => (key ? setConfirming(true) : reloadAndStart())}
            >
              <RefreshCw aria-hidden />
              Reload the page
            </Button>
          )
        }
      >
        {/* The page's header says how old its figures are. */}
        {status.updatedAt === null ? "Nothing has loaded yet." : null}
      </Banner>
      {confirming ? (
        <div className="border-b px-gutter py-3">
          <RestartConfirm
            className="mx-auto max-w-page"
            label="Confirm reloading the page"
            message="The page reloads and the node starts again."
            confirmLabel="Reload the page"
            role={role}
            onCancel={() => setConfirming(false)}
          />
        </div>
      ) : null}
    </>
  );
}

/** The running role's dot in the nav: grey while the node doesn't answer,
 * as nothing on the page is live then. */
function RoleDot() {
  const now = useNow();
  const view = useSyncView(now);
  return (
    <span
      className={
        view.key === "NoAnswer"
          ? "ml-auto size-2 rounded-full bg-muted-foreground/60"
          : "ml-auto size-2 rounded-full bg-status-live"
      }
      aria-label="on in this tab"
    />
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
        // The caption beside the wordmark, behind a hairline: one line in
        // the header-high brand row.
        <div className="flex items-center gap-3">
          <Link to="/" className={`w-fit shrink-0 rounded-md ${focusRing}`}>
            <Logo className="text-[20px]" markClassName="size-7" />
          </Link>
          <span className="border-l pl-3 font-mono text-[0.625rem] leading-none tracking-wider whitespace-nowrap text-muted-foreground uppercase">
            Web node
          </span>
        </div>
      }
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
          {navGroups().map(([group, items]) => (
            <SidebarNavGroup key={group} label={group}>
              {items.map(({ to, label, icon: Icon, page, also }) => (
                <SidebarNavItem
                  key={to}
                  asChild
                  current={route.page === page || !!also?.includes(route.page)}
                >
                  <Link to={to}>
                    <Icon aria-hidden />
                    {label}
                    {roleOf(page) === role ? <RoleDot /> : null}
                  </Link>
                </SidebarNavItem>
              ))}
            </SidebarNavGroup>
          ))}
          <SidebarNavGroup label="Help">
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
          </SidebarNavGroup>
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
