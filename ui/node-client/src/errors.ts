/**
 * How a request to a node fails, in kinds a UI can act on.
 */

import { obj, str } from "./json";

export type NodeErrorKind =
  /** No answer: the node is down, restarting or unreachable. */
  | "offline"
  /** The node doesn't serve this route: an older build, a public node, or a
   * feature compiled out. */
  | "absent"
  /** The node refused the request, with its reason (a feature turned off,
   * a bad input). */
  | "refused"
  /** The node answered with an error. */
  | "failed"
  /** The answer wasn't what the client expects. */
  | "invalid";

export class NodeError extends Error {
  readonly kind: NodeErrorKind;
  readonly status: number | null;
  constructor(
    kind: NodeErrorKind,
    message: string,
    status: number | null = null,
  ) {
    super(message);
    this.kind = kind;
    this.status = status;
  }
}

/** The node's own words for an error: `{"error": "..."}`, a JSON string, or
 * plain text. */
export function errorMessage(body: string): string | null {
  const text = body.trim();
  if (!text) return null;
  try {
    const value: unknown = JSON.parse(text);
    if (typeof value === "string") return value;
    const message = str(obj(value)?.error);
    if (message) return message;
  } catch {
    // Not JSON: plain text.
  }
  return text.length > 300 ? `${text.slice(0, 300)}…` : text;
}

/** The kind of a failed HTTP answer. */
export function errorKind(status: number): NodeErrorKind {
  if (status === 404) return "absent";
  if (status === 400 || status === 403 || status === 405 || status === 415) {
    return "refused";
  }
  return "failed";
}

/** An error's message, whatever threw it. */
export function describeError(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

/** The error's kind; `failed` for anything that isn't a NodeError. */
export function kindOf(error: unknown): NodeErrorKind {
  return error instanceof NodeError ? error.kind : "failed";
}

/** The request got no usable answer: the node is down or erroring (as
 * opposed to refusing on purpose, or not having the route). */
export function isUnreachable(error: unknown): boolean {
  return (
    error instanceof NodeError &&
    (error.kind === "offline" || error.kind === "failed")
  );
}
