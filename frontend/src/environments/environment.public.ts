import { MinaEnv } from '@shared/types/core/environment/mina-env.type';
import { PUBLIC_FEATURES } from '@shared/helpers/public-api';

/** Native public observer behind a same-origin /api proxy (R29). */
export const environment: Readonly<MinaEnv> = {
  production: true,
  identifier: 'Mina public observer',
  publicApi: true,
  controls: { producer: false, send: false },
  canAddNodes: false,
  hidePeersPill: true,
  hideTxPill: true,
  globalConfig: { features: PUBLIC_FEATURES },
  configs: [{ name: 'Public node', url: '/api' }],
};
