/** The node's documentation on minanode.org. Every docs link builds on it. */
export const DOCS_BASE_URL = 'https://minanode.org/docs/';

/** The docs pages that exist, by slug. */
export type DocsSlug =
  | 'block-producer'
  | 'bug-reports'
  | 'build-from-source'
  | 'dashboard'
  | 'install'
  | 'quickstart'
  | 'requirements'
  | 'run-a-node'
  | 'run-a-relay'
  | 'snark-worker';

export function docsUrl(slug?: DocsSlug): string {
  return slug ? `${DOCS_BASE_URL}${slug}/` : DOCS_BASE_URL;
}

/** Dashboard pages with a matching docs page: route prefix, docs slug. */
const DOCS_FOR_ROUTE: [string, DocsSlug][] = [
  ['/dashboard', 'dashboard'],
  ['/block-production', 'block-producer'],
  ['/snarks/worker', 'snark-worker'],
];

/** The docs page for a dashboard path (without query), if there is one. */
export function docsUrlForPath(path: string): string | undefined {
  const match = DOCS_FOR_ROUTE.find(
    ([route]) => path === route || path.startsWith(route + '/'),
  );
  return match ? docsUrl(match[1]) : undefined;
}
