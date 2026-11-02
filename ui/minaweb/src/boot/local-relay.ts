import type { BuildConfig } from "./build-config";

/** A downloadable bundle's server supplies the exact relay allowlist. */
export async function localRelayConfig(
  config: BuildConfig,
): Promise<BuildConfig> {
  const response = await fetch("/local-relay.json", { cache: "no-store" });
  if (!response.ok)
    throw new Error("The local server did not supply its relay config");
  const value = await response.json();
  if (
    !Array.isArray(value.relays) ||
    value.relays.length !== 1 ||
    typeof value.relays[0] !== "string" ||
    !validRelayAddress(value.relays[0])
  )
    throw new Error("Invalid local relay config");
  // Do not append the compiled relay or seed list when overriding it.
  return { ...config, relays: value.relays, seedUrls: [] };
}

/** Match the archive runner's WebRTC signalling address contract. */
export function validRelayAddress(value: string): boolean {
  if (/\s/.test(value)) return false;
  const match =
    /^\/(ip4|ip6|dns4|dns6|dns)\/([^/]+)\/tcp\/([0-9]+)\/webrtc\/(https?)\/p2p\/([^/]+)$/.exec(
      value,
    );
  if (!match) return false;
  const [, family, host, port, , peer] = match;
  if (port.length > 5 || Number(port) < 1 || Number(port) > 65535) return false;
  if (family === "ip4") {
    const parts = host.split(".");
    if (
      parts.length !== 4 ||
      !parts.every(p => /^(0|[1-9][0-9]{0,2})$/.test(p) && Number(p) <= 255)
    )
      return false;
  } else if (family === "ip6") {
    // URL parses IPv6 in the browser; reject zones and non-address syntax.
    if (!/^[0-9a-fA-F:.]+$/.test(host) || !host.includes(":")) return false;
    try {
      new URL(`http://[${host}]/`);
    } catch {
      return false;
    }
  } else if (
    host.replace(/\.$/, "").length > 253 ||
    !host
      .replace(/\.$/, "")
      .split(".")
      .every(label =>
        /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/.test(label),
      )
  )
    return false;
  const alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
  if (peer.length !== 52) return false;
  let number = 0n;
  for (const char of peer) {
    const digit = alphabet.indexOf(char);
    if (digit < 0) return false;
    number = number * 58n + BigInt(digit);
  }
  const hex = number.toString(16).padStart(74, "0");
  return (
    peer.startsWith("1") &&
    !peer.startsWith("11") &&
    hex.length === 74 &&
    hex.startsWith("2408011220")
  );
}
