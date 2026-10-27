/**
 * Block age and data freshness, kept apart (R26c): the best tip block's own
 * timestamp, the node's last sync event, and the last status sample this page
 * received are three different clocks. Pure functions, so that they can be
 * tested without Angular (frontend/tests/freshness.test.mjs).
 */

/** The app samples /status every 10 s; three missed samples mean stale. */
export const STATUS_STALE_MS = 30_000;

/**
 * Mesa mainnet has 90 s slots and a block about every three minutes on
 * average, so gaps of several minutes are normal. Past this, the page says
 * no new block has been seen, without calling it a node fault.
 */
export const TIP_QUIET_MS = 15 * 60_000;

/**
 * A node starts with the genesis block as its best tip and reports Synced
 * until a peer's better tip arrives (seen in R26's builds run: "Synced" at
 * #548147 for about 10 s). A tip older than this, or this many blocks below a
 * peer's, is not the chain's tip, so the page does not call the node synced.
 */
export const SYNCED_TIP_MAX_AGE_MS = 60 * 60_000;
export const SYNCED_MAX_BEHIND_PEERS = 10;

export type FreshnessTone = 'ok' | 'aware' | 'warn';

export interface FreshnessInput {
  /**
   * 'Pending' | 'Connecting' | 'Listening' | 'Bootstrap' | 'Catchup' |
   * 'Synced' | 'Offline'. Since F-dash3 the node says Connecting or Listening,
   * not Synced, until a peer has sent it a best tip.
   */
  status: string;
  blockHeight?: number | null;
  /** The best tip block's own timestamp, ms since the epoch. */
  bestTipTimestamp?: number | null;
  /** When the node's sync state last changed, ms since the epoch. */
  syncTime?: number | null;
  /** The height the node is syncing to, while not synced. */
  targetHeight?: number | null;
  /** When this page last received a status answer, ms since the epoch. */
  sampledAt?: number | null;
  /** The highest best tip a connected peer reports. */
  maxPeerHeight?: number | null;
}

export interface Freshness {
  tipAgeMs: number | null;
  syncEventAgeMs: number | null;
  sampleAgeMs: number | null;
  /** No status answer for STATUS_STALE_MS, or the node is unreachable. */
  sampleStale: boolean;
  /** Synced, but no new best tip for TIP_QUIET_MS. */
  tipQuiet: boolean;
  /** Peers report a higher tip; null when no peer tip is known. */
  behindPeers: number | null;
  /**
   * The highest peer tip minus ours, signed: below zero when no peer reports
   * a tip as high as ours (seen after a relay reconnects, when it is known
   * only by the genesis block until it sends its tip). Null when unknown.
   */
  peerDelta: number | null;
  /**
   * The node says Synced and its tip is plausibly the chain's: not older
   * than SYNCED_TIP_MAX_AGE_MS, not SYNCED_MAX_BEHIND_PEERS below a peer, and
   * at least one of the two known. A node that reports no block timestamp (m6
   * and older) and no peer tip may still be on the genesis block.
   */
  synced: boolean;
}

/** Nanoseconds (as the node reports times) to milliseconds. */
export function nanosToMs(ns: number | null | undefined): number | null {
  if (ns === null || ns === undefined || !Number.isFinite(ns) || ns <= 0) {
    return null;
  }
  return Math.floor(ns / 1_000_000);
}

function age(now: number, at: number | null | undefined): number | null {
  if (at === null || at === undefined || !Number.isFinite(at)) {
    return null;
  }
  // A clock a little ahead of ours is shown as zero, not a negative age.
  return Math.max(0, now - at);
}

export function freshness(input: FreshnessInput, now: number): Freshness {
  const tipAgeMs = age(now, input.bestTipTimestamp);
  const sampleAgeMs = age(now, input.sampledAt);
  const peerDelta =
    input.maxPeerHeight && input.blockHeight
      ? input.maxPeerHeight - input.blockHeight
      : null;
  const behind = peerDelta === null ? null : Math.max(0, peerDelta);
  const synced =
    input.status === 'Synced' &&
    (tipAgeMs !== null || behind !== null) &&
    !(tipAgeMs !== null && tipAgeMs > SYNCED_TIP_MAX_AGE_MS) &&
    !(behind !== null && behind > SYNCED_MAX_BEHIND_PEERS);
  return {
    tipAgeMs,
    syncEventAgeMs: age(now, input.syncTime),
    sampleAgeMs,
    sampleStale:
      input.status === 'Offline' ||
      (sampleAgeMs !== null && sampleAgeMs > STATUS_STALE_MS),
    tipQuiet: synced && tipAgeMs !== null && tipAgeMs > TIP_QUIET_MS,
    behindPeers: behind,
    peerDelta,
    synced,
  };
}

/** The status to show: the node's, except Synced on a tip that cannot be. */
export function shownStatus(input: FreshnessInput, now: number): string {
  if (input.status === 'Synced' && !freshness(input, now).synced) {
    return 'Not synced';
  }
  return (
    {
      Pending: 'Starting',
      Connecting: 'Connecting',
      Listening: 'Waiting for peers',
      Bootstrap: 'Bootstrapping',
      Catchup: 'Catching up',
    }[input.status] ?? input.status
  );
}

/** How the highest peer tip compares with ours, in words. */
export function peerNote(f: Freshness): string {
  const d = f.peerDelta;
  if (d === null) return 'no peer tip to compare';
  if (d === 0) return 'level with our best tip';
  const n = Math.abs(d);
  const blocks = `${n} block${n === 1 ? '' : 's'}`;
  return d > 0
    ? `${blocks} above ours`
    : `${blocks} below ours: peer tip not known yet`;
}

/** "45 s", "6 min 30 s", "2 h 5 min", "3 d 4 h". */
export function formatAge(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) {
    return '–';
  }
  const s = Math.floor(ms / 1000);
  if (s < 60) {
    return `${s} s`;
  }
  const m = Math.floor(s / 60);
  if (m < 60) {
    return s % 60 ? `${m} min ${s % 60} s` : `${m} min`;
  }
  const h = Math.floor(m / 60);
  if (h < 24) {
    return m % 60 ? `${h} h ${m % 60} min` : `${h} h`;
  }
  const d = Math.floor(h / 24);
  return h % 24 ? `${d} d ${h % 24} h` : `${d} d`;
}

/**
 * A block height, one format on the dashboard (F-dash2): '#578486', or '–'
 * when unknown. Without digit grouping: '#578,486' overflows the sync card's
 * Best tip on a 390 px phone.
 */
export function formatHeight(height: number | null | undefined): string {
  if (
    height === null ||
    height === undefined ||
    !Number.isFinite(height) ||
    height <= 0
  ) {
    return '–';
  }
  return '#' + height;
}

/** HH:MM:SS UTC, or '–'. */
export function utcTime(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) {
    return '–';
  }
  return new Date(ms).toISOString().slice(11, 19) + ' UTC';
}

export interface StatusChip {
  /** e.g. 'Synced', 'Bootstrap', 'Offline' */
  label: string;
  /** e.g. '#564682', 'to #564686' */
  height: string;
  /** e.g. 'block 6 min old', 'no answer for 45 s' */
  detail: string;
  tone: FreshnessTone;
}

/**
 * The header chip. While bootstrapping or catching up, the node's best tip is
 * the genesis block or an old root, so the chip shows the target height, not
 * that block. Only a synced node gets the best tip's height and block age.
 */
export function statusChip(input: FreshnessInput, now: number): StatusChip {
  const f = freshness(input, now);
  const height = (h: number | null | undefined, prefix = '') =>
    h ? prefix + formatHeight(h) : '';
  if (input.status === 'Offline') {
    return {
      label: 'Offline',
      height: height(input.blockHeight),
      detail:
        f.sampleAgeMs !== null
          ? `last answer ${formatAge(f.sampleAgeMs)} ago`
          : 'no answer',
      tone: 'warn',
    };
  }
  if (input.status === 'Synced') {
    const stale = f.sampleStale
      ? `no answer for ${formatAge(f.sampleAgeMs)}`
      : '';
    const tip = f.tipAgeMs !== null ? `block ${formatAge(f.tipAgeMs)} old` : '';
    if (!f.synced) {
      // At start, the genesis block until a peer's tip arrives.
      const why =
        f.tipAgeMs !== null && f.tipAgeMs > SYNCED_TIP_MAX_AGE_MS
          ? tip
          : f.behindPeers !== null
            ? `peers at ${formatHeight(input.maxPeerHeight)}`
            : 'no block time or peer tip to check';
      return {
        label: 'Not synced',
        height: height(input.blockHeight),
        detail: stale || why,
        tone: f.sampleStale ? 'warn' : 'aware',
      };
    }
    return {
      label: 'Synced',
      height: height(input.blockHeight),
      detail: stale || tip,
      tone: f.sampleStale ? 'warn' : f.tipQuiet ? 'aware' : 'ok',
    };
  }
  if (input.status === 'Bootstrap' || input.status === 'Catchup') {
    return {
      label: input.status === 'Bootstrap' ? 'Bootstrapping' : 'Catching up',
      height: height(input.targetHeight, 'to '),
      detail: f.sampleStale
        ? `no answer for ${formatAge(f.sampleAgeMs)}`
        : 'not synced yet',
      tone: f.sampleStale ? 'warn' : 'aware',
    };
  }
  if (input.status === 'Connecting' || input.status === 'Listening') {
    return {
      label: input.status === 'Connecting' ? 'Connecting' : 'Waiting for peers',
      height: '',
      detail: f.sampleStale
        ? `no answer for ${formatAge(f.sampleAgeMs)}`
        : input.status === 'Connecting'
          ? 'no peer yet'
          : 'no best tip from peers yet',
      tone: f.sampleStale ? 'warn' : 'aware',
    };
  }
  return { label: 'Starting', height: '', detail: '', tone: 'aware' };
}
