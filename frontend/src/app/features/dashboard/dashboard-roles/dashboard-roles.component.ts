import {
  ChangeDetectionStrategy,
  Component,
  computed,
  DestroyRef,
  inject,
  signal,
} from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { RouterLink } from '@angular/router';
import { Store } from '@ngrx/store';
import {
  catchError,
  exhaustMap,
  filter,
  map,
  of,
  switchMap,
  take,
  timeout,
  timer,
} from 'rxjs';
import { AppSelectors } from '@app/app.state';
import { MinaState } from '@app/app.setup';
import { RustService } from '@core/services/rust.service';
import { CONFIG, isSubFeatureEnabled } from '@shared/constants/config';
import {
  formatAge,
  formatHeight,
  utcTime,
} from '@shared/helpers/freshness.helper';
import { AppNodeDetails } from '@shared/types/app/app-node-details.type';
import { MinaNode } from '@shared/types/core/environment/mina-env.type';
import { ChainValueComponent } from '@app/features/chain/chain-value.component';
import {
  explorerBlockUrl,
  formatMina,
  SnarksWorkerService,
  workerError,
} from '@app/features/snarks/worker/snarks-worker.service';
import {
  IncludedWork,
  WorkerControl,
} from '@app/features/snarks/worker/snarks-worker.types';
import {
  explorerNetwork,
  ProducerStats,
  producerView,
  workerView,
} from './dashboard-roles.helper';

const PRODUCER_EVERY_MS = 10_000;
const CONTROL_EVERY_MS = 10_000;
const INCLUDED_EVERY_MS = 30_000;

/**
 * What the node does for the network (F-dash2): producing blocks and proving
 * SNARK work, on or off, with the key facts for each. Reads the node's
 * `/stats/block_producer`, `/snarker/control` and `/snarker/included`.
 */
@Component({
  selector: 'mina-dashboard-roles',
  imports: [RouterLink, ChainValueComponent],
  templateUrl: './dashboard-roles.component.html',
  styleUrl: './dashboard-roles.component.scss',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class DashboardRolesComponent {
  private readonly rust = inject(RustService);
  private readonly worker = inject(SnarksWorkerService);

  readonly publicApi = CONFIG.publicApi;
  readonly formatHeight = formatHeight;
  readonly formatMina = formatMina;
  readonly formatAge = formatAge;
  readonly utcTime = utcTime;

  readonly node = signal<MinaNode | null>(null);
  readonly details = signal<AppNodeDetails | null>(null);
  readonly stats = signal<ProducerStats | null>(null);
  readonly statsError = signal<string | null>(null);
  readonly control = signal<WorkerControl | null>(null);
  readonly controlError = signal<string | null>(null);
  readonly included = signal<IncludedWork | null>(null);
  readonly now = signal(Date.now());

  readonly producer = computed(() =>
    producerView(
      this.details()?.hasBlockProducer,
      this.stats(),
      this.statsError(),
      this.now(),
    ),
  );
  readonly snarker = computed(() =>
    workerView(
      this.control(),
      this.included(),
      this.controlError(),
      this.now(),
      this.workerLink(),
    ),
  );
  /** The node's own network name if it says, else the one its chain id is. */
  readonly network = computed(() =>
    explorerNetwork(this.included()?.network ?? this.details()?.network),
  );
  readonly wonSlotsLink = computed(() => {
    const node = this.node();
    return !!node && isSubFeatureEnabled(node, 'block-production', 'won-slots');
  });
  readonly workerLink = computed(() => {
    const node = this.node();
    return !!node && isSubFeatureEnabled(node, 'snarks', 'worker');
  });

  constructor() {
    const destroyRef = inject(DestroyRef);
    if (this.publicApi) {
      return;
    }
    inject(Store<MinaState>)
      .select(AppSelectors.activeNodeDetails)
      .pipe(filter(Boolean), takeUntilDestroyed(destroyRef))
      .subscribe(details => this.details.set(details));
    timer(0, 1000)
      .pipe(takeUntilDestroyed(destroyRef))
      .subscribe(() => this.now.set(Date.now()));

    this.worker.node$
      .pipe(
        switchMap(node => {
          this.node.set(node);
          this.stats.set(null);
          this.statsError.set(null);
          return timer(0, PRODUCER_EVERY_MS).pipe(
            // A node that says it has no producer key is not asked.
            filter(() => this.details()?.hasBlockProducer !== false),
            exhaustMap(() =>
              this.rust
                .get<ProducerStats | null>('/stats/block_producer', node)
                .pipe(
                  take(1),
                  timeout(15000),
                  map(stats => ({ stats, error: null as string | null })),
                  catchError(error =>
                    of({ stats: null, error: workerError(error) }),
                  ),
                ),
            ),
          );
        }),
        takeUntilDestroyed(destroyRef),
      )
      .subscribe(({ stats, error }) => {
        this.stats.set(stats);
        this.statsError.set(error);
      });

    this.worker.node$
      .pipe(
        switchMap(node => {
          this.control.set(null);
          this.controlError.set(null);
          if (node.isWebNode) {
            return of({
              control: null,
              error: 'A web node does not run a SNARK worker.',
            });
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
      .subscribe(({ control, error }) => {
        this.control.set(control);
        this.controlError.set(error);
      });

    this.worker.node$
      .pipe(
        switchMap(node => {
          this.included.set(null);
          return node.isWebNode
            ? of(null)
            : timer(0, INCLUDED_EVERY_MS).pipe(
                exhaustMap(() =>
                  this.worker
                    .included(node)
                    .pipe(catchError(() => of(null as IncludedWork | null))),
                ),
              );
        }),
        takeUntilDestroyed(destroyRef),
      )
      .subscribe(included => this.included.set(included));
  }

  explorerUrl(height: number): string | null {
    const network = this.network();
    return network ? explorerBlockUrl(network, height) : null;
  }

  /** "in 2 h 5 min", or "now" for a slot that has started. */
  inTime(at: number): string {
    const ms = at - this.now();
    return ms <= 0 ? 'now' : 'in ' + formatAge(ms);
  }

  count(n: number): string {
    return n.toLocaleString('en-US');
  }
}
