// Read token values from the live CSS and measure contrast the way the
// browser paints it. Runs in the browser only.

export type Theme = "dark" | "light";

let canvas: HTMLCanvasElement | null = null;

function context() {
  canvas ??= document.createElement("canvas");
  canvas.width = canvas.height = 1;
  return canvas.getContext("2d", { willReadFrequently: true })!;
}

/** Paint layers bottom first onto white and read back the sRGB pixel. */
export function paint(layers: string[]): [number, number, number] {
  const ctx = context();
  ctx.clearRect(0, 0, 1, 1);
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(0, 0, 1, 1);
  for (const colour of layers) {
    ctx.fillStyle = colour;
    ctx.fillRect(0, 0, 1, 1);
  }
  const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data;
  return [r, g, b];
}

/** A colour expression's computed value inside a theme scope. */
export function computed(theme: Theme, expression: string) {
  const host = document.createElement("div");
  host.className = theme;
  host.style.cssText = "position:absolute;visibility:hidden;pointer-events:none";
  const probe = document.createElement("div");
  probe.style.color = expression;
  host.appendChild(probe);
  document.body.appendChild(host);
  const value = getComputedStyle(probe).color;
  host.remove();
  return value;
}

/** A custom property's value inside a theme scope, with var() substituted. */
export function rawToken(theme: Theme, name: string) {
  const host = document.createElement("div");
  host.className = theme;
  host.style.cssText = "position:absolute;visibility:hidden";
  document.body.appendChild(host);
  const value = getComputedStyle(host).getPropertyValue(name).trim();
  host.remove();
  return value;
}

export function toHex([r, g, b]: [number, number, number]) {
  return `#${[r, g, b].map(c => c.toString(16).padStart(2, "0")).join("")}`;
}

function luminance([r, g, b]: [number, number, number]) {
  const lin = (c: number) => {
    c /= 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

export function contrast(
  a: [number, number, number],
  b: [number, number, number],
) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** "--brand@0.12" -> a CSS colour for that token at 12% opacity. */
export function layerExpression(spec: string) {
  const [token, alpha] = spec.split("@");
  return alpha
    ? `color-mix(in oklab, var(${token}) ${Number(alpha) * 100}%, transparent)`
    : `var(${token})`;
}
