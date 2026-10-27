import { environment as local } from './environment.local';
import { MinaEnv } from '@shared/types/core/environment/mina-env.type';
import { BUILD_FRONTEND_CONTROLS } from '@shared/constants/build';

/**
 * Read-only dashboard for the native mainnet node. View-only unless the build
 * enables controls with MINA_FRONTEND_CONTROLS (`producer`, `send`, `all`).
 */
export const environment: Readonly<MinaEnv> = {
  ...local,
  production: true,
  controls: BUILD_FRONTEND_CONTROLS,
  globalConfig: {
    ...local.globalConfig,
    features: {
      ...local.globalConfig.features,
      blocks: [],
      accounts: [],
    },
  },
  configs: [{ name: 'Native mainnet', url: 'http://127.0.0.1:3200' }],
};
