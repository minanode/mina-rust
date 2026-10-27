/**
 * The web node's STUN and TURN servers (W1): a JSON list in the browser's
 * RTCIceServer format, fetched at start from the URL the build names
 * (MINA_WEBNODE_ICE_SERVERS_URL), so that a deployment changes its TURN
 * server or credentials without a rebuild:
 *
 *   [{ "urls": "stun:turn.example.org:3478" },
 *    { "urls": ["turn:turn.example.org:3478", "turns:turn.example.org:5349"],
 *      "username": "webnode", "credential": "..." }]
 *
 * A list the node would refuse is dropped here instead, so that a bad file
 * leaves the node on its built-in STUN servers rather than stopping it.
 */
export interface IceServer {
  urls: string[];
  username?: string;
  credential?: string;
}

/**
 * The servers in `json`, keeping only the fields the node reads, or the
 * reason they can't be used: not a list, a server without `stun:`,
 * `stuns:`, `turn:` or `turns:` URLs, or a TURN server without a username
 * and credential. An empty list is valid: no servers.
 */
export function parseIceServers(json: unknown): IceServer[] | string {
  if (!Array.isArray(json)) return 'not a list';
  const servers: IceServer[] = [];
  for (const entry of json) {
    if (typeof entry !== 'object' || entry === null) return 'not an object';
    const { urls, username, credential } = entry as Record<string, unknown>;
    const list = typeof urls === 'string' ? [urls] : urls;
    if (
      !Array.isArray(list) ||
      list.length === 0 ||
      !list.every(url => typeof url === 'string')
    ) {
      return 'a server without urls';
    }
    for (const url of list as string[]) {
      const scheme = url.split(':', 1)[0];
      if (
        !['stun', 'stuns', 'turn', 'turns'].includes(scheme) ||
        !url.includes(':')
      ) {
        return `not a STUN or TURN URL: ${url}`;
      }
      const isTurn = scheme === 'turn' || scheme === 'turns';
      if (
        isTurn &&
        (typeof username !== 'string' || typeof credential !== 'string')
      ) {
        return `TURN server without a username and credential: ${url}`;
      }
    }
    const server: IceServer = { urls: [...(list as string[])] };
    if (typeof username === 'string') server.username = username;
    if (typeof credential === 'string') server.credential = credential;
    servers.push(server);
  }
  return servers;
}

/**
 * Fetches the list at `url`, relative to `base`. Null, and the node keeps
 * its built-in STUN servers, when there is no URL, or it is malformed, or
 * the list can't be fetched or used; the console says why, never with the
 * credentials.
 */
export async function loadIceServers(
  url: string | undefined,
  base: string,
  fetchFn: (input: string) => Promise<Response> = input => fetch(input),
): Promise<IceServer[] | null> {
  if (!url) return null;
  let absolute = url;
  try {
    // A malformed URL falls back too, rather than stopping the start.
    absolute = new URL(url, base).toString();
    const response = await fetchFn(absolute);
    if (!response.ok) {
      console.warn(
        `WebNode: ICE servers ${absolute}: HTTP ${response.status}; using the built-in STUN servers`,
      );
      return null;
    }
    const servers = parseIceServers(await response.json());
    if (typeof servers === 'string') {
      console.warn(
        `WebNode: ICE servers ${absolute}: ${servers}; using the built-in STUN servers`,
      );
      return null;
    }
    console.log(
      `WebNode: ICE servers from ${absolute}:`,
      servers.flatMap(server => server.urls).join(' ') || 'none',
    );
    return servers;
  } catch (error) {
    console.warn(
      `WebNode: ICE servers ${absolute}: ${error}; using the built-in STUN servers`,
    );
    return null;
  }
}
