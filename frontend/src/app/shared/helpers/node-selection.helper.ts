/**
 * Which configured node a page load starts on (R28, after R27 found that
 * reloading a native block page sent the visitor to the web node's start
 * page). Pure, so that it can be tested without Angular
 * (frontend/tests/node-selection.test.mjs).
 */

export interface SelectableNode {
  name: string;
  isWebNode?: boolean;
}

/** The first path segment of a URL: the feature a page belongs to. */
export function pageOf(href: string): string {
  return new URL(href, 'http://x').pathname.split('/')[1] ?? '';
}

/**
 * The node named by the URL's `node` parameter; else, for a page the first
 * node cannot show (a native node's blocks or accounts, say), the first node
 * that can; else the first node.
 */
export function initialNode<T extends SelectableNode>(
  nodes: T[],
  href: string,
  supports: (node: T, page: string) => boolean,
): T | undefined {
  const url = new URL(href, 'http://x');
  const named = nodes.find(n => n.name === url.searchParams.get('node'));
  if (named) {
    return named;
  }
  const page = pageOf(href);
  if (page && nodes.length && !supports(nodes[0], page)) {
    return nodes.find(n => supports(n, page)) ?? nodes[0];
  }
  return nodes[0];
}

/**
 * Whether a fresh page load must first go to the web node's start page:
 * only when the node it starts on is the web node and that is not running.
 */
export function needsWebNodeStart(
  node: SelectableNode | undefined,
  webNodeLoaded: boolean,
): boolean {
  return !!node?.isWebNode && !webNodeLoaded;
}
