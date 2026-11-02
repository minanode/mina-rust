/**
 * A small history router for the page's few routes. Any static host that
 * falls back to index.html serves them (serve -s, Cloudflare Pages).
 */

import * as React from "react";

export type Route =
  | { page: "overview" }
  | { page: "blocks" }
  | { page: "block"; id: string }
  | { page: "accounts"; publicKey: string | null }
  | { page: "pool" }
  | { page: "peers" }
  | { page: "boot" }
  | { page: "key" }
  | { page: "send" }
  | { page: "worker" }
  | { page: "producer" }
  | { page: "not-found"; path: string };

export function matchRoute(pathname: string): Route {
  const path =
    pathname.endsWith("/") && pathname.length > 1
      ? pathname.slice(0, -1)
      : pathname;
  switch (path) {
    case "":
    case "/":
      return { page: "overview" };
    case "/blocks":
      return { page: "blocks" };
    case "/peers":
      return { page: "peers" };
    case "/start":
      return { page: "boot" };
    case "/accounts":
      return { page: "accounts", publicKey: null };
    case "/pool":
      return { page: "pool" };
    case "/key":
      return { page: "key" };
    case "/send":
      return { page: "send" };
    case "/snark-worker":
      return { page: "worker" };
    case "/block-producer":
      return { page: "producer" };
  }
  const segment = (prefix: string) => {
    if (!path.startsWith(prefix)) return null;
    const rest = path.slice(prefix.length);
    if (!rest || rest.includes("/")) return null;
    try {
      return decodeURIComponent(rest);
    } catch {
      return null;
    }
  };
  const block = segment("/blocks/");
  if (block !== null) return { page: "block", id: block };
  const account = segment("/accounts/");
  if (account !== null) return { page: "accounts", publicKey: account };
  return { page: "not-found", path: pathname };
}

const listeners = new Set<() => void>();

/** The dashboard's pages that minaweb shows navigate with their own
 * router, which sends this (minanode-dashboard's NAVIGATE_EVENT). */
const DASHBOARD_NAVIGATE = "minanode:navigate";

function subscribe(listener: () => void) {
  listeners.add(listener);
  window.addEventListener("popstate", listener);
  window.addEventListener(DASHBOARD_NAVIGATE, listener);
  return () => {
    listeners.delete(listener);
    window.removeEventListener("popstate", listener);
    window.removeEventListener(DASHBOARD_NAVIGATE, listener);
  };
}

export function navigate(to: string) {
  if (to === window.location.pathname) return;
  window.history.pushState(null, "", to);
  window.scrollTo({ top: 0 });
  for (const listener of listeners) listener();
}

export function usePathname(): string {
  return React.useSyncExternalStore(
    subscribe,
    () => window.location.pathname,
    () => "/",
  );
}

/** An in-app link: a real `<a href>` that navigates without a reload, which
 * would stop the node. */
export function Link({
  to,
  onClick,
  ...props
}: Omit<React.ComponentProps<"a">, "href"> & { to: string }) {
  return (
    <a
      href={to}
      onClick={event => {
        onClick?.(event);
        if (
          event.defaultPrevented ||
          event.button !== 0 ||
          event.metaKey ||
          event.ctrlKey ||
          event.shiftKey ||
          event.altKey ||
          props.target
        ) {
          return;
        }
        event.preventDefault();
        navigate(to);
      }}
      {...props}
    />
  );
}
