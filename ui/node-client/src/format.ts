/** One format for each kind of value, on every page. */

const NANOMINA = 1_000_000_000n;

function toBigInt(value: string | number | bigint | null | undefined) {
  if (typeof value === "bigint") return value >= 0n ? value : null;
  if (typeof value === "number") {
    return Number.isSafeInteger(value) && value >= 0 ? BigInt(value) : null;
  }
  if (typeof value === "string" && /^\d+$/.test(value)) return BigInt(value);
  return null;
}

/**
 * Nanomina as MINA, exactly (no rounding through floating point), with digit
 * groups in the whole part and no trailing zeros: `1,000`, `0.004`. Null for
 * a value that isn't a non-negative integer.
 */
export function formatMina(
  nanomina: string | number | bigint | null | undefined,
): string | null {
  const amount = toBigInt(nanomina);
  if (amount === null) return null;
  const whole = (amount / NANOMINA).toLocaleString("en-US");
  const fraction = (amount % NANOMINA)
    .toString()
    .padStart(9, "0")
    .replace(/0+$/, "");
  return fraction ? `${whole}.${fraction}` : whole;
}

/** A MINA decimal string as the node writes fees (`"0.01"`) to nanomina. */
export function minaToNanomina(mina: string): bigint | null {
  const match = /^(\d+)(?:\.(\d{1,9}))?$/.exec(mina.trim());
  if (!match) return null;
  const whole = BigInt(match[1]);
  const fraction = BigInt((match[2] ?? "").padEnd(9, "0"));
  return whole * NANOMINA + fraction;
}

/** `#578486`, or `–` when unknown. No digit groups, as in the Angular app. */
export function formatHeight(height: number | null | undefined): string {
  if (height === null || height === undefined || !Number.isFinite(height)) {
    return "–";
  }
  return `#${height}`;
}

/** `45 s`, `6 min 30 s`, `2 h 5 min`, `3 d 4 h`; `–` when unknown. */
export function formatAge(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return "–";
  const s = Math.floor(Math.max(0, ms) / 1000);
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return s % 60 ? `${m} min ${s % 60} s` : `${m} min`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${h} h ${m % 60} min` : `${h} h`;
  const d = Math.floor(h / 24);
  return h % 24 ? `${d} d ${h % 24} h` : `${d} d`;
}

/** `0.8 s`, `12 s`, `1 min 5 s`: how long something took. */
export function formatDuration(ms: number): string {
  const s = Math.max(0, ms) / 1000;
  if (s < 10) return `${s.toFixed(1)} s`;
  if (s < 60) return `${Math.round(s)} s`;
  const whole = Math.round(s);
  const m = Math.floor(whole / 60);
  return whole % 60 ? `${m} min ${whole % 60} s` : `${m} min`;
}

/**
 * A time from a moment, such as a stage from its slot's start: `+3.9 s`,
 * `−5.8 s`, and past a minute `44 min 10 s before` or `2 min after`.
 */
export function formatOffset(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return "–";
  const size = formatDuration(Math.abs(ms));
  if (Math.abs(ms) >= 60_000) return `${size} ${ms < 0 ? "before" : "after"}`;
  return `${ms < 0 ? "−" : "+"}${size}`;
}

/** `HH:MM:SS UTC`, or `–`. */
export function utcTime(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return "–";
  return `${new Date(ms).toISOString().slice(11, 19)} UTC`;
}

/** `2026-10-23 16:09 UTC`, or `–`. */
export function utcDateTime(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return "–";
  const iso = new Date(ms).toISOString();
  return `${iso.slice(0, 10)} ${iso.slice(11, 16)} UTC`;
}

/** `1,234`. */
export function formatCount(n: number | null | undefined): string {
  if (n === null || n === undefined || !Number.isFinite(n)) return "–";
  return n.toLocaleString("en-US");
}

/** The ends of a long hash or key: `B62qp3…TNXb2t`. */
export function shortHash(
  value: string | null | undefined,
  head = 6,
  tail = 6,
): string {
  if (!value) return "–";
  if (value.length <= head + tail + 1) return value;
  return `${value.slice(0, head)}…${value.slice(-tail)}`;
}

/** `1 block`, `3 blocks`. */
export function plural(n: number, one: string, many = `${one}s`): string {
  return `${formatCount(n)} ${n === 1 ? one : many}`;
}
