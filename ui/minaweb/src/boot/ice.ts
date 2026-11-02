/**
 * The web node's STUN and TURN servers: a JSON list in the browser's
 * RTCIceServer format, fetched at start from the URL the build names, so a
 * deployment changes its TURN server without a rebuild. Ported from the
 * Angular page (frontend/src/app/shared/helpers/ice-servers.helper.ts): a
 * list the node would refuse is dropped here, and the node keeps its
 * built-in STUN servers rather than stopping.
 */

export interface IceServer {
  urls: string[];
  username?: string;
  credential?: string;
}

/** Supported RFC 7064/7065 ICE URLs; keep ice-url-cases.json in sync. */
export function validIceUrl(value: unknown): boolean {
  if (typeof value !== "string" || /\s/.test(value)) return false;
  const match =
    /^(stun|stuns|turn|turns):([^?]+)(?:\?transport=(udp|tcp))?$/.exec(value);
  if (!match || (match[1].startsWith("stun") && match[3])) return false;
  const authority =
    /^(\[[0-9a-fA-F:.]+\]|[A-Za-z0-9.-]+)(?::([0-9]{1,5}))?$/.exec(match[2]);
  if (!authority) return false;
  const [, host, port] = authority;
  if (port && (Number(port) < 1 || Number(port) > 65535)) return false;
  if (host.startsWith("[")) {
    try {
      new URL(`http://${host}/`);
    } catch {
      return false;
    }
  } else {
    const name = host.endsWith(".") ? host.slice(0, -1) : host;
    if (
      name.length > 253 ||
      !name
        .split(".")
        .every(label =>
          /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/.test(label),
        )
    )
      return false;
  }
  return true;
}

/** The servers in `json`, keeping only the fields the node reads, or why
 * they can't be used. An empty list is valid: no servers. */
export function parseIceServers(json: unknown): IceServer[] | string {
  if (!Array.isArray(json)) return "not a list";
  const servers: IceServer[] = [];
  for (const entry of json) {
    if (typeof entry !== "object" || entry === null) return "not an object";
    const { urls, username, credential } = entry as Record<string, unknown>;
    const all = typeof urls === "string" ? [urls] : urls;
    if (
      !Array.isArray(all) ||
      all.length === 0 ||
      !all.every(url => typeof url === "string")
    ) {
      return "a server without urls";
    }
    for (const url of all as string[]) {
      const scheme = url.split(":", 1)[0];
      if (!validIceUrl(url)) {
        return "not a STUN or TURN URL";
      }
      const isTurn = scheme === "turn" || scheme === "turns";
      if (
        isTurn &&
        (typeof username !== "string" ||
          !username.trim() ||
          typeof credential !== "string" ||
          !credential.trim())
      ) {
        return "TURN server without a username and credential";
      }
    }
    const server: IceServer = { urls: [...(all as string[])] };
    if (typeof username === "string") server.username = username;
    if (typeof credential === "string") server.credential = credential;
    servers.push(server);
  }
  return servers;
}

/** The list at `url`, or null (and a console line why, never with the
 * credentials) when there is none or it can't be used. */
export async function loadIceServers(
  url: string | null,
  base: string,
  fetchFn: (input: string) => Promise<Response> = input =>
    fetch(input, { cache: "no-store", signal: AbortSignal.timeout(10_000) }),
): Promise<IceServer[] | null> {
  if (!url) return null;
  let absolute = url;
  try {
    absolute = new URL(url, base).toString();
    const response = await fetchFn(absolute);
    if (!response.ok) {
      console.warn(`minaweb: ICE issuer returned HTTP ${response.status}`);
      return null;
    }
    const servers = parseIceServers(await response.json());
    if (typeof servers === "string") {
      console.warn("minaweb: ICE issuer returned an invalid server list");
      return null;
    }
    return servers;
  } catch {
    console.warn("minaweb: ICE issuer unavailable");
    return null;
  }
}

/** Earliest coturn REST expiry; static credentials have no scheduled expiry. */
export function iceExpiry(servers: IceServer[]): number | null {
  const expires = servers.flatMap(server => {
    if (!server.urls.some(url => /^turns?:/.test(url))) return [];
    const match = /^(\d+):/.exec(server.username ?? "");
    return match ? [Number(match[1]) * 1000] : [];
  });
  return expires.length ? Math.min(...expires) : null;
}

/** An issuer renewal must retain usable, expiring TURN service. */
export function validIceRenewal(servers: IceServer[]): boolean {
  if (typeof parseIceServers(servers) === "string") return false;
  const turn = servers.filter(server =>
    server.urls.some(url => /^turns?:/.test(url)),
  );
  return (
    turn.length > 0 &&
    turn.every(server => {
      const match = /^([0-9]{1,12}):[A-Za-z0-9]+$/.exec(server.username ?? "");
      const expires = match ? Number(match[1]) * 1000 : 0;
      return expires > Date.now() && expires <= Date.now() + 86_460_000;
    })
  );
}

/** Renew at 80% of the remaining TTL (at most five minutes before expiry).
 * The timer is independent of sync, and visibility wakes suspended tabs.
 * Failed requests back off to one minute; no overlapping fetches or peer restarts.
 */
export function renewIceServers(
  url: string,
  base: string,
  initial: IceServer[] | null,
  apply: (servers: IceServer[]) => void,
  load = loadIceServers,
): () => void {
  let servers = initial;
  let timer: ReturnType<typeof setTimeout>;
  let stopped = false;
  let fetching = false;
  let retry = 1000;
  let due = 0;
  const schedule = (delay: number) => {
    clearTimeout(timer);
    due = Date.now() + delay;
    timer = setTimeout(refresh, delay);
  };
  const next = () => {
    const expiry = servers && iceExpiry(servers);
    if (expiry === null) return; // Static ICE configuration needs no polling.
    const remaining = (expiry ?? Date.now()) - Date.now();
    schedule(
      Math.max(
        1000,
        Math.min(2_147_483_647, remaining - Math.min(300_000, remaining * 0.2)),
      ),
    );
  };
  const refresh = async () => {
    if (stopped || fetching) return;
    fetching = true;
    try {
      const fresh = await load(url, base);
      if (stopped) return;
      if (!fresh || !validIceRenewal(fresh)) {
        schedule(retry);
        retry = Math.min(60_000, retry * 2);
        return;
      }
      apply(fresh);
      servers = fresh;
      retry = 1000;
      next();
    } catch {
      if (!stopped) {
        schedule(retry);
        retry = Math.min(60_000, retry * 2);
      }
    } finally {
      fetching = false;
    }
  };
  const wake = () => {
    if (document.visibilityState === "visible" && Date.now() >= due)
      void refresh();
  };
  document.addEventListener("visibilitychange", wake);
  if (initial) next();
  else schedule(retry);
  return () => {
    stopped = true;
    clearTimeout(timer);
    document.removeEventListener("visibilitychange", wake);
  };
}

/** The servers of several lists, each relay's, as one list for the node. A
 * TURN URL whose credential has expired is left out: the node refuses a
 * renewal holding one, and a relay whose issuer is down keeps its last list
 * here until a fetch succeeds again. */
export function joinIceServers(
  lists: (IceServer[] | null)[],
  now = Date.now(),
): IceServer[] {
  return lists.flatMap(servers =>
    (servers ?? []).flatMap(server => {
      const match = /^(\d+):/.exec(server.username ?? "");
      const urls =
        match && Number(match[1]) * 1000 <= now
          ? server.urls.filter(url => !/^turns?:/.test(url))
          : server.urls;
      return urls.length ? [{ ...server, urls }] : [];
    }),
  );
}

/** The lists that may enter a join. With several issuers, a list must meet
 * the renewal contract the node's transport enforces on every update
 * (validIceRenewal: usable, expiring TURN credentials). One that doesn't, or
 * failed, is held out as null: the node refuses a whole update holding it,
 * so one misbehaving issuer would otherwise block every other issuer's
 * renewals. renewEachIceServers retries a held-out issuer on its own backoff.
 * A single list is taken as it is, so a static file keeps working. */
export function admitIceLists(
  lists: (IceServer[] | null)[],
): (IceServer[] | null)[] {
  if (lists.length <= 1) return lists;
  return lists.map(list => (list && validIceRenewal(list) ? list : null));
}

/** Each list at `urls`, fetched together; null for one that failed. */
export function loadEachIceServers(
  urls: string[],
  base: string,
  load = loadIceServers,
): Promise<(IceServer[] | null)[]> {
  return Promise.all(urls.map(url => load(url, base)));
}

/** Renew each list on its own schedule (renewIceServers) and hand the node
 * all of them joined, so one relay's issuer being down or misbehaving delays
 * only its own credentials. Only admitted lists (admitIceLists) enter the
 * join, at start and at every renewal, which renewIceServers validates the
 * same way; a failed update keeps every issuer's last good list. */
export function renewEachIceServers(
  urls: string[],
  base: string,
  initial: (IceServer[] | null)[],
  apply: (servers: IceServer[]) => void,
  load = loadIceServers,
): () => void {
  const current = admitIceLists(urls.map((_, index) => initial[index] ?? null));
  const stops = urls.map((url, index) =>
    renewIceServers(
      url,
      base,
      current[index],
      fresh => {
        const next = [...current];
        next[index] = fresh;
        apply(joinIceServers(next));
        current[index] = fresh;
      },
      load,
    ),
  );
  return () => stops.forEach(stop => stop());
}
