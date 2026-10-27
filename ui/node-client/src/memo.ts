/**
 * Transaction memos as the node sends them: Base58Check of a version byte
 * (0x14) and 34 bytes, a tag (1 for text, 0 for a digest), a length and the
 * bytes. OCaml: src/lib/mina_base/signed_command_memo.ml.
 */

const ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

function base58Decode(text: string): Uint8Array | null {
  let value = 0n;
  for (const char of text) {
    const digit = ALPHABET.indexOf(char);
    if (digit < 0) return null;
    value = value * 58n + BigInt(digit);
  }
  const bytes: number[] = [];
  while (value > 0n) {
    bytes.unshift(Number(value % 256n));
    value /= 256n;
  }
  for (const char of text) {
    if (char !== "1") break;
    bytes.unshift(0);
  }
  return Uint8Array.from(bytes);
}

/**
 * The memo's text; a digest memo as `0x` and hex. Null if it isn't a memo.
 * The checksum is not verified: the node already parsed the memo.
 */
export function decodeMemo(memo: string | null | undefined): string | null {
  if (!memo || memo.length > 64) return null;
  const bytes = base58Decode(memo);
  // Version, tag, length, 32 bytes of data, 4 of checksum.
  if (!bytes || bytes.length !== 39 || bytes[0] !== 0x14) return null;
  const tag = bytes[1];
  const length = bytes[2];
  const data = bytes.slice(3, 35);
  if (tag === 0) {
    return `0x${Array.from(data, b => b.toString(16).padStart(2, "0")).join("")}`;
  }
  if (tag !== 1 || length > 32) return null;
  return new TextDecoder("utf-8", { fatal: false }).decode(
    data.slice(0, length),
  );
}
