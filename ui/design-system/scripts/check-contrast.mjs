// Check the WCAG 2.2 AA colour pairs in src/tokens/contrast-pairs.json against
// src/styles/tokens.css, for both themes. Exits 1 if any pair fails.
//
// The parser covers what tokens.css uses: hex values, var() references and
// color-mix(in oklab, <colour> N%, transparent), which is the colour at N%
// opacity. Translucent layers are composited in sRGB, as browsers do.
import { readFileSync } from "node:fs";

const here = new URL("..", import.meta.url);
const css = readFileSync(new URL("src/styles/tokens.css", here), "utf8")
  .replace(/\/\*[\s\S]*?\*\//g, "");
const { pairs } = JSON.parse(
  readFileSync(new URL("src/tokens/contrast-pairs.json", here), "utf8"),
);

function block(selector) {
  const re = new RegExp(`(^|\\n)\\s*${selector}\\s*\\{([^}]*)\\}`, "g");
  const vars = {};
  for (const m of css.matchAll(re)) {
    for (const d of m[2].matchAll(/(--[\w-]+)\s*:\s*([^;]+);/g)) {
      vars[d[1]] = d[2].trim();
    }
  }
  return vars;
}

const primitives = block("@theme static");
const themes = {
  light: { ...primitives, ...block(":root,\\s*\\.light") },
  dark: { ...primitives, ...block("\\.dark") },
};

function hexToRgb(hex) {
  const h = hex.replace("#", "");
  return [0, 2, 4].map(i => parseInt(h.slice(i, i + 2), 16));
}

// Returns [r, g, b, a] with r, g, b in 0..255.
function resolve(vars, value, depth = 0) {
  if (depth > 10) throw new Error(`reference loop at ${value}`);
  value = value.trim();
  if (value.startsWith("#")) return [...hexToRgb(value), 1];
  const ref = value.match(/^var\((--[\w-]+)\)$/);
  if (ref) {
    if (!(ref[1] in vars)) throw new Error(`undefined ${ref[1]}`);
    return resolve(vars, vars[ref[1]], depth + 1);
  }
  const mix = value.match(
    /^color-mix\(in oklab,\s*(.+?)\s+([\d.]+)%,\s*transparent\)$/,
  );
  if (mix) {
    const [r, g, b, a] = resolve(vars, mix[1], depth + 1);
    return [r, g, b, a * (Number(mix[2]) / 100)];
  }
  throw new Error(`cannot resolve ${value}`);
}

function layer(vars, spec) {
  const [token, alpha] = spec.split("@");
  const [r, g, b, a] = resolve(vars, `var(${token})`);
  return [r, g, b, a * (alpha ? Number(alpha) : 1)];
}

function over(top, bottom) {
  const a = top[3];
  return [0, 1, 2].map(i => top[i] * a + bottom[i] * (1 - a)).concat(1);
}

function luminance([r, g, b]) {
  const lin = c => {
    c /= 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

function ratio(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

let failures = 0;
for (const [theme, vars] of Object.entries(themes)) {
  console.log(`\n${theme}`);
  for (const pair of pairs) {
    let bg = [255, 255, 255, 1];
    for (const spec of pair.bg) bg = over(layer(vars, spec), bg);
    const fg = over(layer(vars, pair.fg), bg);
    const r = ratio(fg, bg);
    const ok = r >= pair.min;
    if (!ok) failures++;
    console.log(
      `  ${ok ? "pass" : "FAIL"}  ${r.toFixed(2).padStart(5)}:1  (min ${pair.min})  ${pair.label}`,
    );
  }
}
console.log(failures ? `\n${failures} pair(s) below AA` : "\nall pairs meet AA");
process.exit(failures ? 1 : 0);
