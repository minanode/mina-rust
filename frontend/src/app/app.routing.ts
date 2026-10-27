import { NgModule } from '@angular/core';
import { NoPreloading, RouterModule, Routes } from '@angular/router';
import { CONFIG, getFirstFeature } from '@shared/constants/config';
import { WebNodeLandingPageComponent } from '@app/layout/web-node-landing-page/web-node-landing-page.component';
import { getMergedRoute, MergedRoute } from '@mina-rust/shared';
import { filter, take } from 'rxjs';
import { landingPageGuard } from '@shared/guards/landing-page.guard';
import { PUBLIC_ROUTES } from '@shared/helpers/public-api';
import { controlsGuard } from '@shared/guards/controls.guard';

const APP_TITLE: string = 'minanode';

export const DASHBOARD_TITLE: string = APP_TITLE + ' - Dashboard';
export const RESOURCES_TITLE: string = APP_TITLE + ' - Resources';
export const NETWORK_TITLE: string = APP_TITLE + ' - Network';
export const NODES_TITLE: string = APP_TITLE + ' - Nodes';
export const STATE_TITLE: string = APP_TITLE + ' - State';
export const SNARKS_TITLE: string = APP_TITLE + ' - Snarks';
export const BLOCK_PRODUCTION_TITLE: string = APP_TITLE + ' - Block Production';
export const MEMPOOL_TITLE: string = APP_TITLE + ' - Mempool';
export const BENCHMARKS_TITLE: string = APP_TITLE + ' - Benchmarks';
export const WEBNODE_TITLE: string = APP_TITLE + ' - Web Node';
export const FUZZING_TITLE: string = APP_TITLE + ' - Fuzzing';

export function generateRoutes(): Routes {
  const routes: Routes = [
    {
      path: 'blocks',
      loadComponent: () =>
        import('@app/features/chain/blocks.component').then(
          m => m.BlocksComponent,
        ),
      title: APP_TITLE + ' - Blocks',
    },
    {
      path: 'blocks/:stateHash',
      loadComponent: () =>
        import('@app/features/chain/block-detail.component').then(
          m => m.BlockDetailComponent,
        ),
      title: APP_TITLE + ' - Block',
    },
    {
      path: 'accounts',
      loadComponent: () =>
        import('@app/features/chain/accounts.component').then(
          m => m.AccountsComponent,
        ),
      title: APP_TITLE + ' - Accounts',
    },
    {
      path: 'dashboard',
      loadChildren: () =>
        import('@dashboard/dashboard.module').then(m => m.DashboardModule),
      title: DASHBOARD_TITLE,
      canActivate: [landingPageGuard],
    },
    {
      path: 'nodes',
      loadChildren: () =>
        import('@nodes/nodes.module').then(m => m.NodesModule),
      title: NODES_TITLE,
      // canActivate: [FeatureGuard],
    },
    {
      path: 'resources',
      loadChildren: () =>
        import('@resources/resources.module').then(m => m.ResourcesModule),
      title: RESOURCES_TITLE,
    },
    {
      path: 'network',
      loadChildren: () =>
        import('@network/network.module').then(m => m.NetworkModule),
      title: NETWORK_TITLE,
    },
    {
      path: 'state',
      loadChildren: () =>
        import('@state/state.module').then(m => m.StateModule),
      title: STATE_TITLE,
      canActivate: [landingPageGuard],
    },
    {
      path: 'snarks',
      loadChildren: () =>
        import('@snarks/snarks.module').then(m => m.SnarksModule),
      title: SNARKS_TITLE,
    },
    {
      path: 'block-production',
      loadChildren: () =>
        import('@block-production/block-production.module').then(
          m => m.BlockProductionModule,
        ),
      title: BLOCK_PRODUCTION_TITLE,
      canActivate: [controlsGuard('producer'), landingPageGuard],
    },
    {
      path: 'mempool',
      loadChildren: () =>
        import('@mempool/mempool.module').then(m => m.MempoolModule),
      title: MEMPOOL_TITLE,
      canActivate: [landingPageGuard],
    },
    {
      path: 'benchmarks',
      loadChildren: () =>
        import('@benchmarks/benchmarks.module').then(m => m.BenchmarksModule),
      title: BENCHMARKS_TITLE,
      canActivate: [controlsGuard('send'), landingPageGuard],
    },
    {
      path: 'fuzzing',
      loadChildren: () =>
        import('@fuzzing/fuzzing.module').then(m => m.FuzzingModule),
      title: FUZZING_TITLE,
    },
    {
      path: 'loading-web-node',
      loadChildren: () =>
        import('@web-node/web-node.module').then(m => m.WebNodeModule),
      title: WEBNODE_TITLE,
      canActivate: [landingPageGuard],
    },
  ];
  if (CONFIG.showWebNodeLandingPage) {
    routes.push({
      path: '',
      component: WebNodeLandingPageComponent,
    });
  }

  return [
    ...(CONFIG.publicApi
      ? routes.filter(route => PUBLIC_ROUTES.includes(route.path))
      : routes),
    {
      path: '**',
      redirectTo: CONFIG.showWebNodeLandingPage ? '' : getFirstFeature(),
      pathMatch: 'full',
    },
  ];
}

@NgModule({
  imports: [
    RouterModule.forRoot(generateRoutes(), {
      // enableTracing: true,
      preloadingStrategy: NoPreloading,
      onSameUrlNavigation: 'ignore',
      initialNavigation: 'enabledNonBlocking',
    }),
  ],
  exports: [RouterModule],
})
export class AppRouting {}
