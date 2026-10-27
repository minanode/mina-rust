/** The node's `GET /snarker/control` (P5.4). Fees are in nanomina. */
export interface WorkerControl {
  /** The node was started with `--allow-worker-control`. */
  allowed: boolean;
  enabled: boolean;
  public_key: string | null;
  fee: number | null;
  account_creation_fee: number;
  worker: WorkerStatus;
  job: WorkerJob | null;
  /** Newest first. */
  recent: WorkerOutcome[];
  paused_for_block: boolean;
  best_tip_height: number | null;
  zkapp_jobs: number;
  /** Whether the key's account exists at the best tip; null if unknown. */
  account_exists: boolean | null;
  /** Why OCaml's snark pool would refuse work at this fee from this key. */
  fee_problem: string | null;
  /** Why the setting saved in the work directory was not used at startup. */
  setting_warning?: string | null;
  /** Proofs the worker returned since the node started (F-dash2). */
  proofs_made?: number;
}

export interface WorkerStatus {
  kind:
    | 'None'
    | 'Starting'
    | 'Idle'
    | 'Working'
    | 'WorkReady'
    | 'WorkError'
    | 'Cancelling'
    | 'Cancelled'
    | 'Error'
    | 'Killing';
  job_id?: string;
  error?: unknown;
  permanent?: boolean;
}

export interface WorkerJob {
  job_id: string;
  summary: { Tx?: number; Merge?: number };
  zkapp: boolean;
  /** Nanoseconds since the Unix epoch. */
  submitted: number;
}

export interface WorkerOutcome {
  job_id: string;
  summary: { Tx?: number; Merge?: number } | null;
  zkapp: boolean;
  /** Nanoseconds since the Unix epoch. */
  time: number;
  duration_ms: number | null;
  fee: number | null;
  offered: boolean;
  error: string | null;
}

/** The node's `GET /snarker/included` (P5.4). */
export interface IncludedWork {
  network: string;
  keys: string[];
  started_ms: number | null;
  final_totals: IncludedWorkTotals;
  pending_totals: IncludedWorkTotals;
  max_blocks: number;
  /** Newest first. */
  entries: IncludedWorkEntry[];
}

export interface IncludedWorkTotals {
  blocks: number;
  bundles: number;
  zkapp_bundles: number;
  fees: number;
}

export type IncludedWorkStatus = 'pending' | 'final' | 'dropped' | 'unknown';

export interface IncludedWorkEntry {
  height: number;
  state_hash: string;
  time_ms: number;
  producer: string;
  prover: string;
  bundles: number;
  zkapp_bundles: number;
  fees: number;
  status: IncludedWorkStatus;
}

export interface WorkerChange {
  enabled: boolean;
  public_key?: string;
  /** In MINA, as `--snarker-fee` takes it. */
  fee?: string;
}
