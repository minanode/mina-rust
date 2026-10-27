import {
  ChangeDetectionStrategy,
  Component,
  DestroyRef,
  inject,
  signal,
} from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { HttpErrorResponse } from '@angular/common/http';
import { CommonModule } from '@angular/common';
import { catchError, exhaustMap, map, of, switchMap, timer } from 'rxjs';
import { MinaNode } from '@shared/types/core/environment/mina-env.type';
import { ChainValueComponent } from '@app/features/chain/chain-value.component';
import {
  explorerBlockUrl,
  formatMina,
  SnarksWorkerService,
  workerError,
} from './snarks-worker.service';
import {
  IncludedWork,
  IncludedWorkStatus,
  WorkerChange,
  WorkerControl,
  WorkerOutcome,
} from './snarks-worker.types';

const CONTROL_EVERY_MS = 5000;
const LOG_EVERY_MS = 15000;

/**
 * SNARKs → Worker (P5.4): turn the native node's SNARK worker on or off,
 * set its key and fee, watch its jobs, and see the blocks that bought its
 * work. Controls show only when the node allows them.
 */
@Component({
  selector: 'mina-snarks-worker',
  imports: [CommonModule, ChainValueComponent],
  templateUrl: './snarks-worker.component.html',
  styleUrl: './snarks-worker.component.scss',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class SnarksWorkerComponent {
  private readonly worker = inject(SnarksWorkerService);
  readonly formatMina = formatMina;
  readonly explorerBlockUrl = explorerBlockUrl;

  readonly node = signal<MinaNode | null>(null);
  readonly control = signal<WorkerControl | null>(null);
  readonly controlError = signal<string | null>(null);
  readonly included = signal<IncludedWork | null>(null);
  readonly includedError = signal<string | null>(null);
  readonly now = signal(Date.now());

  /** The form: the key and the fee in MINA. */
  readonly publicKey = signal('');
  readonly fee = signal('');
  /** The form was edited since it was filled from the node. */
  private edited = false;
  readonly busy = signal(false);
  readonly changeError = signal<string | null>(null);
  readonly changeDone = signal<string | null>(null);

  constructor() {
    const destroyRef = inject(DestroyRef);
    this.worker.node$
      .pipe(
        switchMap(node => {
          this.node.set(node);
          this.control.set(null);
          this.included.set(null);
          this.edited = false;
          if (node.isWebNode) {
            return of(null);
          }
          return timer(0, CONTROL_EVERY_MS).pipe(
            exhaustMap(() =>
              this.worker.control(node).pipe(
                map(control => ({ control, error: null as string | null })),
                catchError(error =>
                  of({ control: null, error: workerError(error) }),
                ),
              ),
            ),
          );
        }),
        takeUntilDestroyed(destroyRef),
      )
      .subscribe(result => {
        if (!result) {
          return;
        }
        this.controlError.set(result.error);
        if (result.control) {
          this.setControl(result.control);
        }
      });
    this.worker.node$
      .pipe(
        switchMap(node =>
          node.isWebNode
            ? of(null)
            : timer(0, LOG_EVERY_MS).pipe(
                exhaustMap(() =>
                  this.worker.included(node).pipe(
                    map(log => ({ log, error: null as string | null })),
                    catchError(error =>
                      of({ log: null, error: workerError(error) }),
                    ),
                  ),
                ),
              ),
        ),
        takeUntilDestroyed(destroyRef),
      )
      .subscribe(result => {
        if (!result) {
          return;
        }
        this.includedError.set(result.error);
        if (result.log) {
          this.included.set(result.log);
        }
      });
    timer(0, 1000)
      .pipe(takeUntilDestroyed(destroyRef))
      .subscribe(() => this.now.set(Date.now()));
  }

  private setControl(control: WorkerControl): void {
    this.control.set(control);
    if (!this.edited) {
      this.publicKey.set(control.public_key ?? '');
      this.fee.set(control.fee !== null ? formatMina(control.fee) : '');
    }
  }

  onKey(value: string): void {
    this.edited = true;
    this.publicKey.set(value.trim());
  }

  onFee(value: string): void {
    this.edited = true;
    this.fee.set(value.trim());
  }

  /** The form differs from what the worker runs with. */
  get changed(): boolean {
    const control = this.control();
    return (
      !!control &&
      (this.publicKey() !== (control.public_key ?? '') ||
        this.fee() !== (control.fee !== null ? formatMina(control.fee) : ''))
    );
  }

  toggle(): void {
    const control = this.control();
    if (!control) {
      return;
    }
    if (control.enabled) {
      this.send({ enabled: false }, 'The worker is stopped.');
    } else {
      this.start();
    }
  }

  start(): void {
    this.send(
      { enabled: true, public_key: this.publicKey(), fee: this.fee() },
      this.control()?.enabled
        ? 'The worker runs with the new setting.'
        : 'The worker is starting.',
    );
  }

  private send(change: WorkerChange, done: string): void {
    const node = this.node();
    if (!node || this.busy()) {
      return;
    }
    this.busy.set(true);
    this.changeError.set(null);
    this.changeDone.set(null);
    this.worker.change(node, change).subscribe({
      next: control => {
        this.busy.set(false);
        this.edited = false;
        this.setControl(control);
        this.changeDone.set(done);
      },
      error: error => {
        this.busy.set(false);
        // A browser reports a change refused at its preflight as no answer.
        const refused =
          error instanceof HttpErrorResponse && error.status === 0
            ? ` If the node runs, it may refuse changes from this page: start it with --worker-control-origin ${location.origin}.`
            : '';
        this.changeError.set(workerError(error) + refused);
      },
    });
  }

  workerState(control: WorkerControl): string {
    switch (control.worker.kind) {
      case 'None':
        return control.enabled ? 'Starting soon' : 'Off';
      case 'Starting':
        return 'Starting';
      case 'Idle':
        return control.paused_for_block
          ? 'Waiting: a block of ours is due'
          : 'Waiting for a job';
      case 'Working':
        return 'Proving';
      case 'WorkReady':
      case 'WorkError':
      case 'Cancelled':
        return 'Finishing a job';
      case 'Cancelling':
        return 'Cancelling a job';
      case 'Killing':
        return 'Stopping (a proof in flight finishes first)';
      case 'Error':
        return 'Stopped on an error';
    }
  }

  jobKind(job: {
    zkapp: boolean;
    summary?: { Tx?: number; Merge?: number } | null;
  }): string {
    if (job.zkapp) {
      return 'zkApp command';
    }
    if (!job.summary) {
      return 'bundle';
    }
    return job.summary.Merge !== undefined ? 'merge' : 'transaction';
  }

  outcomeLabel(outcome: WorkerOutcome): string {
    if (outcome.error) {
      return 'failed';
    }
    return outcome.offered ? 'offered' : 'dropped (setting changed)';
  }

  seconds(ms: number | null): string {
    return ms === null ? '–' : (ms / 1000).toFixed(1) + ' s';
  }

  since(nanoseconds: number): string {
    const seconds = Math.max(
      0,
      Math.round((this.now() - nanoseconds / 1e6) / 1000),
    );
    if (seconds < 120) {
      return `${seconds} s`;
    }
    return `${Math.round(seconds / 60)} min`;
  }

  statusLabel(status: IncludedWorkStatus): string {
    return status;
  }

  blocks(count: number): string {
    return count === 1 ? '1 block' : `${count.toLocaleString()} blocks`;
  }

  failures(control: WorkerControl): WorkerOutcome[] {
    return control.recent.filter(outcome => !!outcome.error);
  }

  proofTimes(control: WorkerControl): string {
    const times = control.recent
      .filter(outcome => !outcome.error && outcome.duration_ms !== null)
      .map(outcome => outcome.duration_ms as number)
      .sort((a, b) => a - b);
    if (!times.length) {
      return '–';
    }
    const middle = Math.floor(times.length / 2);
    const median =
      times.length % 2
        ? times[middle]
        : (times[middle - 1] + times[middle]) / 2;
    return `${this.seconds(median)} median of ${times.length}`;
  }

  workerError(control: WorkerControl): string | null {
    if (control.worker.kind !== 'Error') {
      return null;
    }
    return JSON.stringify(control.worker.error);
  }
}
