export type CheckStatus =
  | 'notRun'
  | 'pending'
  | 'passed'
  | 'failed'
  | 'skipped';
export interface LocalCheck {
  status: CheckStatus;
  checkedAt: number | null;
  elapsedMs: number | null;
}
export interface VerificationReceipt {
  block: { stateHash: string; height: number };
  network: string;
  arrival: 'gossip' | 'catchup' | 'bootstrap' | 'bestTip' | 'local';
  /** Not in the public receipt (`verificationReceipts`). */
  receivedAt?: number;
  applied: boolean;
  blockProof: LocalCheck;
  deltaChain: LocalCheck;
  bodyReference: LocalCheck;
  verifierKey: string | null;
}
export interface VerificationSnapshot {
  capacity: number;
  verifiedSinceStart: number;
  failuresSinceStart: number;
  recentFailureCount: number;
  recentFailures: {
    block: { stateHash: string; height: number };
    check: string;
    checkedAt: number;
  }[];
  medianProofTimeMs: number | null;
  lastVerified: VerificationReceipt | null;
  receipts: VerificationReceipt[];
  /** Only in an answer to a lookup by hash (nodes after m6). */
  lookup?: { requested: number; found: number };
}

/** Receipts per request: the node's MAX_RECEIPT_PAGE_SIZE. */
export const RECEIPT_PAGE_SIZE = 128;

/**
 * The node's RECEIPT_CAPACITY. Paging stops here whatever capacity a node
 * reports, so that a node cannot make the page send more than 8 requests.
 */
export const MAX_RECEIPT_CAPACITY = 1024;

function readAll(pages: VerificationSnapshot[]): boolean {
  const last = pages[pages.length - 1];
  const capacity = Number.isFinite(last.capacity)
    ? Math.min(last.capacity, MAX_RECEIPT_CAPACITY)
    : MAX_RECEIPT_CAPACITY;
  return (
    last.receipts.length < RECEIPT_PAGE_SIZE ||
    pages.length * RECEIPT_PAGE_SIZE >= capacity
  );
}

/**
 * Receipts found for a set of state hashes. `complete` means every retained
 * receipt was searched, so a hash missing from `receipts` has none retained.
 */
export interface ReceiptLookupResult {
  receipts: Map<string, VerificationReceipt>;
  complete: boolean;
  /** 'hash' for a node that answers lookups, 'pages' for an older one. */
  method: 'hash' | 'pages';
  requests: number;
}

/** The answer of a node that looked the hashes up itself. */
export function lookupResult(
  snapshot: VerificationSnapshot,
): ReceiptLookupResult {
  return {
    receipts: new Map(snapshot.receipts.map(r => [r.block.stateHash, r])),
    complete: true,
    method: 'hash',
    requests: 1,
  };
}

/**
 * For a node that ignores `hashes` (m6 and older): whether another page is
 * needed, given the pages so far. Pages hold receipts newest first; the
 * search stops once every hash is found, a page comes back short, or the
 * node's capacity, at most MAX_RECEIPT_CAPACITY, has been read: at most
 * MAX_RECEIPT_CAPACITY / RECEIPT_PAGE_SIZE (8) requests.
 */
export function needsAnotherPage(
  hashes: string[],
  pages: VerificationSnapshot[],
): boolean {
  if (pages.length === 0) return true;
  if (readAll(pages)) return false;
  const found = new Set(
    pages.flatMap(page => page.receipts.map(r => r.block.stateHash)),
  );
  return hashes.some(hash => !found.has(hash));
}

export function pagesResult(
  hashes: string[],
  pages: VerificationSnapshot[],
): ReceiptLookupResult {
  const wanted = new Set(hashes);
  const receipts = new Map<string, VerificationReceipt>();
  for (const page of pages) {
    for (const receipt of page.receipts) {
      const hash = receipt.block.stateHash;
      if (wanted.has(hash) && !receipts.has(hash)) receipts.set(hash, receipt);
    }
  }
  const exhausted = pages.length > 0 && readAll(pages);
  return {
    receipts,
    complete: exhausted || receipts.size === wanted.size,
    method: 'pages',
    requests: pages.length,
  };
}

export const NO_RETAINED_RECEIPT = 'No retained receipt';

export function receiptLabel(receipt?: VerificationReceipt): string {
  if (!receipt) return NO_RETAINED_RECEIPT;
  if (receipt.blockProof.status === 'passed') return 'Proof verified here';
  if (receipt.blockProof.status === 'failed') return 'Block proof failed';
  if (receipt.blockProof.status === 'skipped') return 'Proof check skipped';
  if (receipt.blockProof.status === 'pending')
    return 'Proof verification pending';
  if (receipt.applied && receipt.arrival === 'bootstrap')
    return 'Applied from bootstrap, not individually verified';
  if (receipt.applied) return 'Applied, not individually verified';
  return 'Block proof not run';
}
export function checkLabel(check: LocalCheck): string {
  return {
    notRun: 'Not run',
    pending: 'Pending',
    passed: 'Passed',
    failed: 'Failed',
    skipped: 'Skipped',
  }[check.status];
}

/**
 * State hashes per public lookup: the node's MAX_PUBLIC_RECEIPT_HASHES, a page
 * of recent blocks.
 */
export const PUBLIC_RECEIPT_PAGE_SIZE = 20;

/** The public node's receipt lookup (`--public-graphql`), in one request. */
export const PUBLIC_RECEIPTS_QUERY = `query VerificationReceipts($stateHashes: [String!]!) { verificationReceipts(stateHashes: $stateHashes) { stateHash height network arrival applied blockProof { status elapsedMs } deltaChain { status elapsedMs } bodyReference { status elapsedMs } verifierKey } }`;

interface PublicCheck {
  status: CheckStatus;
  elapsedMs: number | null;
}

/** One entry of `verificationReceipts`: the public projection of a receipt. */
export interface PublicVerificationReceipt {
  stateHash: string;
  height: string;
  network: string;
  arrival: VerificationReceipt['arrival'];
  applied: boolean;
  blockProof: PublicCheck;
  deltaChain: PublicCheck;
  bodyReference: PublicCheck;
  verifierKey: string | null;
}

function publicCheck(check: PublicCheck): LocalCheck {
  return {
    status: check.status,
    // Absolute local check/receive times are private.
    checkedAt: null,
    elapsedMs: check.elapsedMs,
  };
}

export function publicReceipt(
  receipt: PublicVerificationReceipt,
): VerificationReceipt {
  return {
    block: { stateHash: receipt.stateHash, height: Number(receipt.height) },
    network: receipt.network,
    arrival: receipt.arrival,
    applied: receipt.applied,
    blockProof: publicCheck(receipt.blockProof),
    deltaChain: publicCheck(receipt.deltaChain),
    bodyReference: publicCheck(receipt.bodyReference),
    verifierKey: receipt.verifierKey,
  };
}

/**
 * A public node's answer: it looked every hash up in all its retained
 * receipts, so a hash it left out has none retained.
 */
export function publicLookupResult(
  receipts: PublicVerificationReceipt[],
): ReceiptLookupResult {
  return {
    receipts: new Map(
      receipts.map(r => [r.stateHash, publicReceipt(r)] as const),
    ),
    complete: true,
    method: 'hash',
    requests: 1,
  };
}

/** Keep long state hashes out of compact receipt rows. */
export function shortReceiptHash(hash: string): string {
  return hash.length > 16 ? `${hash.slice(0, 9)}…${hash.slice(-5)}` : hash;
}
