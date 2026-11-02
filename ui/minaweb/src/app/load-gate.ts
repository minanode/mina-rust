/** The start gate at the page's load (boot/autostart.ts). main.tsx imports
 * this first, so the address loses its nonce before the router or anything
 * else reads it. */

import { loadGate } from "../boot/autostart";

export const gate = loadGate();

// For the headless checks: how this load decided, and when the address was
// cleaned, in ms after the response started (the moment the address shows).
const cleanedAt = performance.now();
const nav = performance.getEntriesByType("navigation")[0] as
  | PerformanceNavigationTiming
  | undefined;
(globalThis as { __minawebAutostart?: unknown }).__minawebAutostart = {
  autostart: gate.autostart,
  param: gate.param,
  stored: gate.stored,
  cleanedAtMs: cleanedAt,
  cleanedAfterResponseMs: nav ? cleanedAt - nav.responseStart : null,
};
