/**
 * A small history router: the dashboard has a handful of pages, all listed
 * in routes.json, which the node also reads to serve index.html for them.
 */

import * as React from "react";

import routes from "../routes.json";

export type Route =
  | { page: "overview" }
  | { page: "worker" }
  | { page: "producer" }
  | { page: "blocks" }
  | { page: "block"; id: string }
  | { page: "account"; publicKey: string | null }
  | { page: "mempool" }
  | { page: "peers" }
  | { page: "status" }
  | { page: "graphql" }
  | { page: "report" }
  | { page: "not-found"; path: string };

export const ROUTE_PATTERNS: readonly string[] = routes.routes;

/** The pattern of routes.json that `pathname` matches, and its `:id`. */
export function matchPattern(
  pathname: string,
): { pattern: string; id: string | null } | null {
  // One trailing slash after a page other than "/", as the node reads it.
  const path =
    pathname.endsWith("/") && pathname.length > 2
      ? pathname.slice(0, -1)
      : pathname;
  for (const pattern of ROUTE_PATTERNS) {
    if (pattern.endsWith("/:id")) {
      const prefix = pattern.slice(0, -":id".length);
      const rest = path.startsWith(prefix) ? path.slice(prefix.length) : "";
      if (rest && !rest.includes("/")) {
        return { pattern, id: decodeURIComponent(rest) };
      }
    } else if (path === pattern) {
      return { pattern, id: null };
    }
  }
  return null;
}

export function matchRoute(pathname: string): Route {
  const match = matchPattern(pathname);
  switch (match?.pattern) {
    case "/":
      return { page: "overview" };
    case "/snark-worker":
      return { page: "worker" };
    case "/block-producer":
      return { page: "producer" };
    case "/blocks":
      return { page: "blocks" };
    case "/blocks/:id":
      return { page: "block", id: match.id ?? "" };
    case "/account":
      return { page: "account", publicKey: null };
    case "/account/:id":
      return { page: "account", publicKey: match.id };
    case "/mempool":
      return { page: "mempool" };
    case "/peers":
      return { page: "peers" };
    case "/node-status":
      return { page: "status" };
    case "/graphql-endpoint":
      return { page: "graphql" };
    case "/report-a-problem":
      return { page: "report" };
    default:
      return { page: "not-found", path: pathname };
  }
}

const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  window.addEventListener("popstate", listener);
  return () => {
    listeners.delete(listener);
    window.removeEventListener("popstate", listener);
  };
}

export function navigate(to: string, { replace = false } = {}) {
  if (to === window.location.pathname + window.location.search) return;
  if (replace) window.history.replaceState(null, "", to);
  else window.history.pushState(null, "", to);
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

/** An in-app link: a real `<a href>`, so it opens in a new tab with a
 * modifier, and navigates without a reload otherwise. */
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
