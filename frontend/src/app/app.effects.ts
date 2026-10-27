import { Injectable } from '@angular/core';
import { MinaState, selectMinaState } from '@app/app.setup';
import { Actions, createEffect, ofType } from '@ngrx/effects';
import { Store } from '@ngrx/store';
import { Effect, removeParamsFromURL } from '@mina-rust/shared';
import {
  catchError,
  filter,
  finalize,
  map,
  of,
  switchMap,
  takeUntil,
  tap,
} from 'rxjs';
import { AppActions } from '@app/app.actions';
import { Router } from '@angular/router';
import {
  FeatureType,
  MinaNode,
} from '@shared/types/core/environment/mina-env.type';
import { AppService } from '@app/app.service';
import { getFirstFeature, isFeatureEnabled } from '@shared/constants/config';
import { RustService } from '@core/services/rust.service';
import { BaseEffect } from '@shared/base-classes/mina-rust-base.effect';
import { WebNodeService } from '@core/services/web-node.service';
import { addError } from '@shared/constants/store-functions';
import { MinaErrorType } from '@shared/types/error-preview/mina-error-type.enum';
import { AppNodeStatus } from '@shared/types/app/app-node-details.type';
import { Routes } from '@shared/enums/routes.enum';

@Injectable()
export class AppEffects extends BaseEffect {
  readonly init$: Effect;
  readonly initSuccess$: Effect;
  readonly onNodeChange$: Effect;
  readonly getNodeDetails$: Effect;
  readonly getNodeEnvBuild$: Effect;

  private requestInProgress: string | undefined;

  constructor(
    private actions$: Actions,
    private appService: AppService,
    private rustNode: RustService,
    private router: Router,
    private webNodeService: WebNodeService,
    store: Store<MinaState>,
  ) {
    super(store, selectMinaState);

    this.init$ = createEffect(() =>
      this.actions$.pipe(
        ofType(AppActions.init),
        switchMap(() => this.appService.getNodes()),
        switchMap((nodes: MinaNode[]) =>
          this.appService.getActiveNode(nodes).pipe(
            tap((activeNode: MinaNode) =>
              this.rustNode.changeRustNode(activeNode),
            ),
            map((activeNode: MinaNode) => ({ activeNode, nodes })),
          ),
        ),
        map((payload: { activeNode: MinaNode; nodes: MinaNode[] }) =>
          AppActions.initSuccess(payload),
        ),
      ),
    );

    this.initSuccess$ = createEffect(() =>
      this.actions$.pipe(
        ofType(AppActions.initSuccess),
        this.latestActionState(),
        switchMap(({ state }) => this.startWebNodeIfSelected(state)),
        map(() => AppActions.getNodeEnvBuild()),
      ),
    );

    // Dispatching: the build identity and status of the node just selected
    // are fetched again (R27 #2: before, they were never refreshed, and the
    // last node's build stayed on screen).
    this.onNodeChange$ = createEffect(() =>
      this.actions$.pipe(
        ofType(AppActions.changeActiveNode),
        this.latestActionState(),
        tap(({ state }) => {
          const node = state.app.activeNode;
          this.rustNode.changeRustNode(node);
          const activePage = removeParamsFromURL(
            this.router.url.split('/')[1],
          ) as FeatureType;
          const queryParams = { node: node.name };
          if (node.isWebNode && !this.webNodeService.started) {
            // A browser node that is not running yet starts from its page.
            this.router.navigate([Routes.LOADING_WEB_NODE], { queryParams });
          } else if (!isFeatureEnabled(node, activePage)) {
            this.router.navigate([getFirstFeature(node)], { queryParams });
          } else {
            this.router.navigate([], {
              queryParams,
              queryParamsHandling: 'merge',
            });
          }
        }),
        switchMap(({ state }) => this.startWebNodeIfSelected(state)),
        switchMap(() => [
          AppActions.getNodeDetails(),
          AppActions.getNodeEnvBuild(),
        ]),
      ),
    );

    // switchMap: only the selected node's answer counts, and each answer
    // carries the name of the node it came from.
    this.getNodeEnvBuild$ = createEffect(() =>
      this.actions$.pipe(
        ofType(AppActions.getNodeEnvBuild),
        switchMap(() => {
          const node = this.rustNode.name;
          return this.appService.getEnvBuild().pipe(
            map(envBuild =>
              AppActions.getNodeEnvBuildSuccess({ envBuild, node }),
            ),
            catchError(() =>
              of(
                AppActions.getNodeEnvBuildSuccess({
                  envBuild: undefined,
                  node,
                }),
              ),
            ),
          );
        }),
      ),
    );

    this.getNodeDetails$ = createEffect(() =>
      this.actions$.pipe(
        ofType(AppActions.getNodeDetails),
        this.latestActionState(),
        // A poll for B must not be suppressed by an outstanding poll for A.
        filter(
          ({ state }) => this.requestInProgress !== state.app.activeNode.name,
        ),
        switchMap(({ state }) => {
          const node = state.app.activeNode;
          this.requestInProgress = node.name;
          return this.appService.getActiveNodeDetails(node).pipe(
            map(details =>
              AppActions.getNodeDetailsSuccess({ details, node: node.name }),
            ),
            catchError(error => [
              addError(error, MinaErrorType.GENERIC),
              AppActions.getNodeDetailsSuccess({
                node: node.name,
                details: {
                  status: AppNodeStatus.OFFLINE,
                  blockHeight: null,
                  blockTime: null,
                  peersConnected: 0,
                  peersDisconnected: 0,
                  peersConnecting: 0,
                  transactions: 0,
                  snarks: 0,
                  producingBlockAt: null,
                  producingBlockGlobalSlot: null,
                  producingBlockStatus: null,
                },
              }),
            ]),
            // Cancel even while a selected web node is still starting.
            takeUntil(this.actions$.pipe(ofType(AppActions.changeActiveNode))),
            finalize(() => (this.requestInProgress = undefined)),
          );
        }),
      ),
    );
  }

  /** Wait for the browser node to start, when it is the selected node. */
  private startWebNodeIfSelected(state: MinaState) {
    return state.app.activeNode.isWebNode
      ? this.webNodeService.startOnce$()
      : of({});
  }
}
