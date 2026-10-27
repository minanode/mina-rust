/**
 * Copies text to the clipboard. The Clipboard API exists only in secure
 * contexts (https, or localhost), and the node dashboard is often opened
 * over plain http at a LAN or Tailscale address, so this falls back to a
 * hidden textarea and `execCommand("copy")`, which browsers still honour
 * there. Resolves to whether the text was copied.
 */
export async function copyText(text: string): Promise<boolean> {
  if (
    typeof window !== "undefined" &&
    window.isSecureContext &&
    navigator.clipboard
  ) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // Denied: try the fallback.
    }
  }
  if (typeof document === "undefined") return false;
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.top = "0";
  area.style.left = "0";
  area.style.opacity = "0";
  const focused = document.activeElement as HTMLElement | null;
  document.body.appendChild(area);
  area.select();
  let copied = false;
  try {
    copied = document.execCommand("copy");
  } catch {
    copied = false;
  }
  area.remove();
  focused?.focus();
  return copied;
}

/** A copy button's name for its state: what it copies, that it did, or
 * that it couldn't (then the text is there to select by hand). */
export function copyName(
  state: "idle" | "copied" | "failed",
  what: string,
): string {
  if (state === "copied") return "Copied";
  if (state === "failed") return "Couldn't copy: select the text instead";
  return `Copy ${what}`;
}
