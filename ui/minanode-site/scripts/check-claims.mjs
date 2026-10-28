// Check the built site against CLAIMS.md: node scripts/check-claims.mjs [out]
//
// 1. Every data-claim id in the built HTML has an entry in CLAIMS.md, and
//    every entry is used on a page.
// 2. Every figure an entry lists appears in the source lines it cites
//    when a local source is supplied; wrapped lines are joined and case ignored.
//    A `coordinator:` source is evidence supplied by the coordinator,
//    with no file to check; its figures are listed without local validation.
//
// This checks ids and figures, not meaning: it cannot tell whether a sentence
// says more than its source. A human or reviewer pass is still needed before
// publishing.
// 3. Built pages load nothing from another origin, and the only external links
//    are to the explorer, release repository, local dashboard or Mina's own
//    docs (plus metadata).
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

const site = resolve(new URL("..", import.meta.url).pathname);
const out = resolve(process.argv[2] ?? join(site, "out"));
const docs = resolve(site, "../../docs");
const allowedLinkHosts = new Set([
  "o1-labs.github.io",
  "minanode.org",
  "localhost:3000",
  "127.0.0.1:3000",
  "github.com",
  "docs.minaprotocol.com",
]);

const problems = [];

function htmlFiles(dir) {
  return readdirSync(dir).flatMap(name => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return htmlFiles(path);
    return name.endsWith(".html") ? [path] : [];
  });
}

// Entries in CLAIMS.md.
const claimsMd = readFileSync(join(site, "CLAIMS.md"), "utf8");
const entries = new Map();
for (const section of claimsMd.split(/^### /m).slice(1)) {
  const id = section.match(/^(C\d+)\b/)?.[1];
  if (!id) continue;
  const field = name => {
    const m = section.match(new RegExp(`^- ${name}:\\s*([\\s\\S]*?)(?=^- |$(?![\\s\\S]))`, "m"));
    return m ? m[1] : "";
  };
  const ticks = text => [...text.matchAll(/`([^`]+)`/g)].map(m => m[1]);
  if (entries.has(id)) problems.push(`${id}: more than one entry in CLAIMS.md`);
  entries.set(id, { sources: ticks(field("Sources")), figures: ticks(field("Figures")) });
}

// Source lines, read on first use.
const sourceLines = {};
function linesOf(file) {
  const path = file.startsWith("research/")
    ? join(docs, file)
    : join(docs, "showcase", file);
  sourceLines[file] ??= readFileSync(path, "utf8").split("\n");
  return sourceLines[file];
}
const coordinatorSources = [];

for (const [id, { sources, figures }] of entries) {
  if (sources.length === 0) problems.push(`${id}: no sources`);
  let text = "";
  for (const ref of sources) {
    if (ref.startsWith("coordinator:")) {
      coordinatorSources.push(`${id}: ${ref}`);
      continue;
    }
    const m = ref.match(
      /^(summary\.md|claims\.md|research\/[\w.-]+\.md):(\d+)(?:-(\d+))?$/,
    );
    if (!m) {
      problems.push(`${id}: cannot read source reference ${ref}`);
      continue;
    }
    const lines = linesOf(m[1]);
    const first = Number(m[2]);
    const last = Number(m[3] ?? m[2]);
    if (last > lines.length || first < 1 || last < first) {
      problems.push(`${id}: ${ref} is outside ${m[1]} (${lines.length} lines)`);
      continue;
    }
    text += " " + lines.slice(first - 1, last).join(" ");
  }
  // Supplied evidence has no local source text to compare its figures with.
  if (sources.some(ref => ref.startsWith("coordinator:"))) continue;
  const haystack = text.replace(/\*\*/g, "").replace(/\s+/g, " ").toLowerCase();
  for (const figure of figures) {
    if (!haystack.includes(figure.toLowerCase())) {
      problems.push(`${id}: figure "${figure}" not found in ${sources.join(", ")}`);
    }
  }
}

// Built pages.
const used = new Set();
const externalLinks = new Map();
for (const file of htmlFiles(out)) {
  const html = readFileSync(file, "utf8");
  for (const m of html.matchAll(/data-claim="([^"]+)"/g)) {
    for (const id of m[1].split(/\s+/)) used.add(id);
  }
  for (const m of html.matchAll(/<(script|link|img|iframe|source)\b[^>]*\b(?:src|href)="(https?:)?\/\/([^"/]+)[^"]*"/g)) {
    const tag = m[0];
    const isMeta = /rel="(canonical|alternate)"/.test(tag);
    if (!isMeta) problems.push(`${file}: loads from another origin: ${tag.slice(0, 120)}`);
  }
  for (const m of html.matchAll(/<a\b[^>]*href="https?:\/\/([^"/]+)[^"]*"/g)) {
    externalLinks.set(m[1], (externalLinks.get(m[1]) ?? 0) + 1);
    if (!allowedLinkHosts.has(m[1])) problems.push(`${file}: link to ${m[1]}`);
  }
}
for (const id of used) if (!entries.has(id)) problems.push(`${id}: on a page but not in CLAIMS.md`);
for (const id of entries.keys()) if (!used.has(id)) problems.push(`${id}: in CLAIMS.md but not on any page`);

console.log(`claims on pages: ${used.size}; entries: ${entries.size}`);
console.log(`external links: ${[...externalLinks].map(([h, n]) => `${h} (${n})`).join(", ") || "none"}`);
if (coordinatorSources.length) {
  console.log(`coordinator-supplied evidence, not checkable here: ${coordinatorSources.join("; ")}`);
}
if (problems.length) {
  console.log(`\n${problems.length} problem(s):`);
  for (const p of problems) console.log(`  ${p}`);
  process.exit(1);
}
console.log("claim IDs and available source checks passed; private measurements are not independently validated");
