import { CONFIG } from '@shared/constants/config';
import { VerificationReceiptComponent } from '../verification/verification-receipt.component';
import { VerificationPanelComponent } from '../verification/verification-panel.component';
import { FreshnessComponent } from '../verification/freshness.component';
import { formatAge } from '@shared/helpers/freshness.helper';
import { ChangeDetectionStrategy, Component, inject } from '@angular/core';
import { CommonModule } from '@angular/common';
import { RouterLink } from '@angular/router';
import {
  catchError,
  exhaustMap,
  map,
  of,
  scan,
  startWith,
  switchMap,
  timer,
} from 'rxjs';
import {
  ChainService,
  queryError,
  WEB_NODE_HAS_NO_GRAPHQL,
} from './chain.service';
import { ChainBlock, QueryState } from './chain.types';
import { blockCounts, commandFees, slotSinceHardFork } from './chain-format';
import { ChainValueComponent } from './chain-value.component';

@Component({
  selector: 'mina-blocks',
  imports: [
    CommonModule,
    RouterLink,
    ChainValueComponent,
    VerificationReceiptComponent,
    VerificationPanelComponent,
    FreshnessComponent,
  ],
  templateUrl: './blocks.component.html',
  styleUrl: './chain.scss',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class BlocksComponent {
  readonly publicApi = CONFIG.publicApi;
  private readonly chain = inject(ChainService);
  readonly counts = blockCounts;
  readonly fees = commandFees;
  readonly forkSlot = slotSinceHardFork;
  readonly formatAge = formatAge;
  readonly noGraphQL = WEB_NODE_HAS_NO_GRAPHQL;
  /** Ticks every second, for the best tip's age. */
  readonly now$ = timer(0, 1000).pipe(map(() => Date.now()));
  readonly vm$ = this.chain.node$.pipe(
    switchMap(node =>
      timer(0, 10000).pipe(
        exhaustMap(() =>
          this.chain.bestChain(node).pipe(
            map(
              data =>
                ({ data, updatedAt: Date.now() }) as QueryState<ChainBlock[]>,
            ),
            catchError(error =>
              of({ error: queryError(error) } as QueryState<ChainBlock[]>),
            ),
          ),
        ),
        // Keep the last good response during a temporary outage, explicitly marked stale.
        scan(
          (previous: QueryState<ChainBlock[]>, next) =>
            next.error
              ? { ...previous, loading: false, error: next.error }
              : next,
          {},
        ),
        startWith({ loading: true } as QueryState<ChainBlock[]>),
      ),
    ),
  );
}
