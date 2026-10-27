import { VerificationReceiptComponent } from '../verification/verification-receipt.component';
import { ChangeDetectionStrategy, Component, inject } from '@angular/core';
import { CommonModule } from '@angular/common';
import { ActivatedRoute, RouterLink } from '@angular/router';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import {
  combineLatest,
  distinctUntilChanged,
  filter,
  map,
  shareReplay,
  switchMap,
} from 'rxjs';
import { ChainService, queryState } from './chain.service';
import {
  blockCounts,
  commandFees,
  findCommand,
  formatMina,
  slotSinceHardFork,
} from './chain-format';
import { ChainValueComponent } from './chain-value.component';
import { decodeMemo } from '@shared/helpers/transaction.helper';
import { ZkappCommand } from './chain.types';

@Component({
  selector: 'mina-block-detail',
  imports: [
    CommonModule,
    RouterLink,
    ChainValueComponent,
    VerificationReceiptComponent,
  ],
  templateUrl: './block-detail.component.html',
  styleUrl: './chain.scss',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class BlockDetailComponent {
  private readonly chain = inject(ChainService);
  private readonly route = inject(ActivatedRoute);
  readonly mina = formatMina;
  readonly counts = blockCounts;
  readonly fees = commandFees;
  readonly find = findCommand;
  readonly forkSlot = slotSinceHardFork;
  readonly vm$ = combineLatest([
    this.chain.node$,
    this.route.paramMap.pipe(
      map(params => params.get('stateHash')!),
      distinctUntilChanged(),
    ),
  ]).pipe(
    switchMap(([node, hash]) => queryState(this.chain.block(node, hash))),
    shareReplay({ bufferSize: 1, refCount: true }),
  );
  /**
   * A transaction's permalink is its block's page with `tx=<hash>`: there
   * is no page for an included transaction of its own (R27 #7).
   */
  readonly tx$ = this.route.queryParamMap.pipe(
    map(params => params.get('tx')),
    distinctUntilChanged(),
  );

  constructor() {
    // Bring the linked transaction into view once the block is shown.
    combineLatest([this.vm$.pipe(filter(vm => !!vm.data)), this.tx$])
      .pipe(
        filter(([, tx]) => !!tx),
        takeUntilDestroyed(),
      )
      .subscribe(([, tx]) =>
        setTimeout(() =>
          document
            .getElementById(`tx-${tx}`)
            ?.scrollIntoView({ block: 'center' }),
        ),
      );
  }

  memo(value: string): string {
    try {
      return decodeMemo(value) || '(empty)';
    } catch {
      return value;
    }
  }

  failures(command: ZkappCommand): string {
    return (
      command.failureReason
        ?.map(
          failure => `Update ${failure.index}: ${failure.failures.join(', ')}`,
        )
        .join('; ') || ''
    );
  }
}
