import { CanActivateFn, Router } from '@angular/router';
import { inject } from '@angular/core';
import { CONFIG, getFirstFeature } from '@shared/constants/config';
import { FrontendControls } from '@shared/types/core/environment/mina-env.type';

/**
 * A view-only build hides block production and sending from the menu; this
 * keeps a direct link to them from opening the page too (R26g).
 */
export function controlsGuard(control: keyof FrontendControls): CanActivateFn {
  return () =>
    CONFIG.controls?.[control]
      ? true
      : inject(Router).createUrlTree(['/' + getFirstFeature()], {
          queryParamsHandling: 'preserve',
        });
}
