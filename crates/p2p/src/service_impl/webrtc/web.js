// https://bugs.chromium.org/p/chromium/issues/detail?id=825576
// workaround: https://stackoverflow.com/questions/66546934/how-to-clear-closed-rtcpeerconnection-with-workaround
export function webrtcCleanup() {
  queueMicrotask(() => {
    console.warn("[WebRTC] doing heavy (around 50ms) GC for dangling peer connections");
    let img = document.createElement("img");
    img.src = window.URL.createObjectURL(new Blob([new ArrayBuffer(5e+7)])); // 50Mo or less or more depending as you wish to force/invoke GC cycle run
    img.onerror = function() {
      window.URL.revokeObjectURL(this.src);
      img = null
    }
  });
}

export function schedulePeriodicWebrtcCleanup() {
  setInterval(webrtcCleanup, 60 * 1000);
}

// P2pTaskRemoteSpawner creates connections in the page's main thread, where
// credential updates from the page use this same module instance.
let renewedIceServers;
const connections = new Set();

function usableServers(servers) {
  return servers.flatMap(server => {
    const expiry = /^(\d+):/.exec(server.username ?? "");
    const urls = typeof server.urls === "string" ? [server.urls] : server.urls;
    const live = expiry && Number(expiry[1]) * 1000 <= Date.now()
      ? urls.filter(url => !/^turns?:/.test(url)) : urls;
    return live.length ? [{ ...server, urls: live }] : [];
  });
}

export function createWebrtcConnection(config) {
  const connection = new RTCPeerConnection({
    ...config,
    iceServers: usableServers(renewedIceServers ?? config.iceServers ?? []),
  });
  connections.add(new WeakRef(connection));
  return connection;
}

/** Supported RFC 7064/7065 ICE URLs; keep ice-url-cases.json in sync. */
export function validIceUrl(value) {
  if (typeof value !== "string" || /\s/.test(value)) return false;
  const match = /^(stun|stuns|turn|turns):([^?]+)(?:\?transport=(udp|tcp))?$/.exec(value);
  if (!match || (match[1].startsWith("stun") && match[3])) return false;
  const authority = /^(\[[0-9a-fA-F:.]+\]|[A-Za-z0-9.-]+)(?::([0-9]{1,5}))?$/.exec(match[2]);
  if (!authority) return false;
  const [, host, port] = authority;
  if (port && (Number(port) < 1 || Number(port) > 65535)) return false;
  if (host.startsWith("[")) {
    try { new URL(`http://${host}/`); } catch { return false; }
  } else {
    const name = host.endsWith(".") ? host.slice(0, -1) : host;
    if (name.length > 253 || !name.split(".").every(label =>
      /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/.test(label))) return false;
  }
  return true;
}

export function updateWebrtcIceServers(servers) {
  const invalid = () => new Error("invalid ICE renewal configuration");
  if (!Array.isArray(servers) || !servers.length) throw invalid();
  let hasTurn = false;
  for (const server of servers) {
    if (!server || typeof server !== "object") throw invalid();
    const urls = typeof server.urls === "string" ? [server.urls] : server.urls;
    if (!Array.isArray(urls) || !urls.length || !urls.every(validIceUrl)) throw invalid();
    if (urls.some(url => /^turns?:/.test(url))) {
      hasTurn = true;
      const match = typeof server.username === "string" && /^([0-9]{1,12}):[A-Za-z0-9]+$/.exec(server.username);
      const expires = match ? Number(match[1]) * 1000 : 0;
      if (expires <= Date.now() || expires > Date.now() + 86_460_000 ||
          typeof server.credential !== "string" || !server.credential.trim()) throw invalid();
    }
  }
  if (!hasTurn) throw invalid();
  // Let this browser validate every URL before changing any working peer or
  // shared defaults. The probe never gathers candidates or opens a channel.
  let fresh;
  try {
    const probe = new RTCPeerConnection({ iceServers: servers });
    try { fresh = structuredClone(probe.getConfiguration().iceServers); }
    finally { probe.close(); }
  } catch { throw invalid(); }
  const changed = [];
  try {
    for (const ref of connections) {
      const connection = ref.deref();
      if (!connection || connection.signalingState === "closed") {
        connections.delete(ref);
        continue;
      }
      const previous = connection.getConfiguration();
      try { connection.setConfiguration({ ...previous, iceServers: fresh }); }
      catch {
        if (connection.signalingState === "closed") { connections.delete(ref); continue; }
        throw invalid();
      }
      changed.push([connection, previous]);
    }
  } catch {
    for (const [connection, previous] of changed) {
      try { connection.setConfiguration(previous); } catch { /* A peer may close. */ }
    }
    throw invalid();
  }
  renewedIceServers = fresh;
}
