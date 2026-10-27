/**
 * Reading the node's JSON without trusting its shape. Nodes of different
 * versions answer with more or fewer fields, so every reader takes `unknown`
 * and returns null for what is missing or of the wrong type, never throws.
 */

export type Json = Record<string, unknown>;

export function obj(value: unknown): Json | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Json)
    : null;
}

export function arr(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

export function str(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

/** A finite number, or a string of one (GraphQL sends heights as strings). */
export function num(value: unknown): number | null {
  if (typeof value === "number") return Number.isFinite(value) ? value : null;
  if (typeof value === "string" && /^-?\d+(\.\d+)?$/.test(value.trim())) {
    return Number(value);
  }
  return null;
}

export function bool(value: unknown): boolean | null {
  return typeof value === "boolean" ? value : null;
}

/** The node's times are nanoseconds since the Unix epoch; null for 0. */
export function nanosToMs(value: unknown): number | null {
  const ns = num(value);
  if (ns === null || ns <= 0) return null;
  return Math.floor(ns / 1_000_000);
}
