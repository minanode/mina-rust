import { ChangeDetectionStrategy, Component, inject } from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { ActivatedRoute, Router } from '@angular/router';
import {
  combineLatest,
  distinctUntilChanged,
  map,
  of,
  startWith,
  Subject,
  switchMap,
  tap,
} from 'rxjs';
import { ChainService, queryState } from './chain.service';
import { LedgerAccount, QueryState } from './chain.types';
import { formatMina } from './chain-format';
import { ChainValueComponent } from './chain-value.component';

@Component({
  selector: 'mina-accounts',
  imports: [CommonModule, FormsModule, ChainValueComponent],
  templateUrl: './accounts.component.html',
  styleUrl: './chain.scss',
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class AccountsComponent {
  private readonly chain = inject(ChainService);
  private readonly route = inject(ActivatedRoute);
  private readonly router = inject(Router);
  publicKey = '';
  readonly mina = formatMina;
  private readonly refresh$ = new Subject<void>();
  readonly vm$ = combineLatest([
    this.chain.node$,
    this.route.queryParamMap.pipe(
      map(params => (params.get('publicKey') || '').trim()),
      distinctUntilChanged(),
      tap(key => (this.publicKey = key)),
    ),
    this.refresh$.pipe(startWith(undefined)),
  ]).pipe(
    switchMap(([node, key]) => {
      if (!key) return of({} as QueryState<LedgerAccount | null>);
      if (!/^B62[1-9A-HJ-NP-Za-km-z]{52}$/.test(key)) {
        return of({
          error:
            'Enter a complete Mina public key (55 base58 characters, starting with B62).',
        } as QueryState<LedgerAccount | null>);
      }
      return queryState(this.chain.account(node, key));
    }),
  );

  lookup(): void {
    if (
      this.publicKey.trim() ===
      this.route.snapshot.queryParamMap.get('publicKey')
    ) {
      this.refresh$.next();
      return;
    }
    this.router.navigate([], {
      relativeTo: this.route,
      queryParams: { publicKey: this.publicKey.trim() || null },
      queryParamsHandling: 'merge',
    });
  }
}
