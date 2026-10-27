import {
  FeaturesConfig,
  FeatureType,
  FrontendControls,
  MinaEnv,
  MinaNode,
} from '@shared/types/core/environment/mina-env.type';
import { PUBLIC_FEATURES } from '@shared/helpers/public-api';
import { environment } from '@environment/environment';
import { ALL_CONTROLS, BUILD_FRONTEND_COMMIT } from '@shared/constants/build';
import {
  getOrigin,
  hasValue,
  isBrowser,
  safelyExecuteInBrowser,
} from '@mina-rust/shared';

const CONTROLS: FrontendControls = environment.controls ?? ALL_CONTROLS;

/** Features that act on the chain, and the control each needs. */
const CONTROL_FEATURES: [FeatureType, keyof FrontendControls][] = [
  ['block-production', 'producer'],
  ['benchmarks', 'send'],
];

/** Drop the features a view-only build must not offer, menu and routes alike. */
export function withoutDisabledControls(
  features: FeaturesConfig | undefined,
  controls: FrontendControls = CONTROLS,
): FeaturesConfig | undefined {
  if (environment.publicApi) {
    return PUBLIC_FEATURES;
  }
  if (!features) {
    return features;
  }
  const allowed = { ...features };
  for (const [feature, control] of CONTROL_FEATURES) {
    if (!controls[control]) {
      delete allowed[feature];
    }
  }
  return allowed;
}

// NOTE: When modifying CONFIG or related functions, update the documentation at:
// website/docs/developers/frontend/environment-configuration.mdx
export const CONFIG: Readonly<MinaEnv> = {
  ...environment,
  controls: CONTROLS,
  frontendCommit: environment.frontendCommit || BUILD_FRONTEND_COMMIT,
  globalConfig: {
    ...environment.globalConfig,
    features: withoutDisabledControls(environment.globalConfig?.features),
    graphQL: getURL(environment.globalConfig?.graphQL),
  },
  configs: !isBrowser()
    ? []
    : environment.configs.map(config => ({
        ...config,
        features: withoutDisabledControls(config.features),
        url: getURL(config.url),
        memoryProfiler: getURL(config.memoryProfiler),
        debugger: getURL(config.debugger),
      })),
};

safelyExecuteInBrowser(() => {
  (window as any).config = CONFIG;
});

export function getAvailableFeatures(config: MinaNode): FeatureType[] {
  const featuresConfig = getFeaturesConfig(config);
  if (!featuresConfig) {
    console.warn('getAvailableFeatures: No features configuration found', {
      config,
      CONFIG,
    });
    return [];
  }
  return Object.keys(featuresConfig) as FeatureType[];
}

export function getFirstFeature(
  config: MinaNode = CONFIG.configs[0],
): FeatureType {
  if (!isBrowser()) {
    return '' as FeatureType;
  }

  console.log('getFirstFeature called with config:', {
    config,
    'CONFIG.configs': CONFIG.configs,
    globalConfig: CONFIG.globalConfig,
  });

  if (Array.isArray(config?.features)) {
    return config.features[0];
  }

  const featuresConfig = getFeaturesConfig(config);
  if (!featuresConfig) {
    console.warn('getFirstFeature: No features configuration found', {
      config,
      CONFIG,
    });
    return '' as FeatureType;
  }
  return Object.keys(featuresConfig)[0] as FeatureType;
}

export function isFeatureEnabled(
  config: MinaNode,
  feature: FeatureType,
): boolean {
  if (Array.isArray(config?.features)) {
    return hasValue(config.features[0]);
  }

  const featuresConfig = getFeaturesConfig(config);
  if (!featuresConfig) {
    console.warn('isFeatureEnabled: No features configuration found', {
      config,
      feature,
    });
    return false;
  }
  return hasValue(featuresConfig[feature]);
}

/**
 * Whether a node can show a page: its features, and never the GraphQL
 * explorer pages (blocks, accounts) for the web node, which has no GraphQL.
 */
export function nodeSupportsPage(config: MinaNode, page: string): boolean {
  if (config?.isWebNode && ['blocks', 'accounts'].includes(page)) {
    return false;
  }
  return isFeatureEnabled(config, page as FeatureType);
}

export function getFeaturesConfig(
  config: MinaNode,
): FeaturesConfig | undefined {
  if (CONFIG.configs.length === 0) {
    return withoutMissingTools(CONFIG.globalConfig?.features, config);
  }
  return withoutMissingTools(
    config?.features || CONFIG.globalConfig?.features,
    config,
  );
}

/** Network tabs that read from the network debugger, not from the node. */
const DEBUGGER_NETWORK_TABS = ['messages', 'connections', 'blocks'];

/**
 * Drop the pages a node can't serve by itself unless it names the tool they
 * read from: the Network tabs that need the network debugger, and Resources,
 * which needs the memory profiler. Without them those pages only show errors.
 */
export function withoutMissingTools(
  features: FeaturesConfig | undefined,
  config: MinaNode | undefined,
): FeaturesConfig | undefined {
  if (!features) {
    return features;
  }
  const allowed = { ...features };
  if (!config?.debugger && allowed.network) {
    allowed.network = allowed.network.filter(
      tab => !DEBUGGER_NETWORK_TABS.includes(tab),
    );
    if (allowed.network.length === 0) {
      delete allowed.network;
    }
  }
  if (!config?.memoryProfiler) {
    delete allowed.resources;
  }
  // SNARKs → Worker came after "Add node" saved its feature lists, so nodes
  // added before it lack the tab. A native node with a SNARKs page gets it;
  // the page asks the node whether control is allowed.
  if (
    !config?.isWebNode &&
    allowed.snarks &&
    !allowed.snarks.includes('worker')
  ) {
    allowed.snarks = ['worker', ...allowed.snarks];
  }
  return allowed;
}

export function isSubFeatureEnabled(
  config: MinaNode,
  feature: FeatureType,
  subFeature: string,
): boolean {
  const features = getFeaturesConfig(config);
  if (!features) {
    console.warn('isSubFeatureEnabled: No features configuration found', {
      config,
      feature,
      subFeature,
    });
    return false;
  }
  return hasValue(features[feature]) && features[feature].includes(subFeature);
}

export function getURL(pathOrUrl: string): string {
  if (isBrowser()) {
    if (pathOrUrl) {
      let href = new URL(pathOrUrl, getOrigin()).href;
      if (href.endsWith('/')) {
        href = href.slice(0, -1);
      }
      return href;
    }
    return pathOrUrl;
  }
  return pathOrUrl;
}
