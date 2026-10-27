/** Links to o1Labs' public explorer, for networks it knows. */

const BASE = "https://o1-labs.github.io/mina-explorer/#";

function network(name: string | null): string | null {
  return name === "devnet" || name === "mainnet" ? name : null;
}

export function explorerBlockUrl(name: string | null, height: number | null) {
  const net = network(name);
  return net && height !== null
    ? `${BASE}/block/${height}?network=${net}`
    : null;
}

export function explorerTransactionUrl(
  name: string | null,
  hash: string | null,
) {
  const net = network(name);
  return net && hash ? `${BASE}/transaction/${hash}?network=${net}` : null;
}
