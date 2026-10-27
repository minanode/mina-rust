// Inspect the static export after each build. React comments do not create spaces.
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join, resolve } from "node:path";

const root = resolve(
  process.argv[2] ?? new URL("../out", import.meta.url).pathname,
);
const problems = [];
const finalRelease = process.env.RELEASE_FINAL === "1";
const pendingSigning = /RELEASE_KEY_PENDING|SIGNING_FINGERPRINT_PENDING/;
const site = resolve(new URL("..", import.meta.url).pathname);
// Keep the served trust file identical to the tracked source, including in previews.
try {
  const source = readFileSync(
    join(site, "../../docs/release/allowed_signers"),
    "utf8",
  );
  const served = readFileSync(join(root, "allowed_signers"), "utf8");
  if (served !== source)
    problems.push("allowed_signers differs from the tracked source");
  if (!pendingSigning.test(source)) {
    const match = source
      .trim()
      .match(
        /^release@minanode\.org[ \t]+ssh-ed25519[ \t]+([A-Za-z0-9+/]+={0,2})(?:[ \t]+[^\r\n]+)?$/,
      );
    if (!match)
      problems.push(
        "allowed_signers must contain one release@minanode.org Ed25519 key",
      );
    else {
      const blob = Buffer.from(match[1], "base64");
      const prefix = Buffer.from(
        "0000000b7373682d6564323535313900000020",
        "hex",
      );
      if (blob.length !== 51 || !blob.subarray(0, 19).equals(prefix))
        problems.push("allowed_signers contains an invalid Ed25519 public key");
      const fingerprint = `SHA256:${createHash("sha256").update(blob).digest("base64").replace(/=+$/, "")}`;
      const configured = readFileSync(
        join(site, "content/release.ts"),
        "utf8",
      ).match(/releaseSigningFingerprint\s*=\s*"([^"]+)"/)?.[1];
      if (configured !== fingerprint)
        problems.push(
          "releaseSigningFingerprint does not match allowed_signers",
        );
    }
  }
} catch (error) {
  problems.push(`cannot check allowed_signers: ${error.message}`);
}
// The compose files the Docker page links to are the repository's own.
for (const file of [
  "docker-compose.yml",
  "docker-compose.block-producer.yml",
]) {
  try {
    if (
      readFileSync(join(root, "docker", file), "utf8") !==
      readFileSync(join(site, "../..", file), "utf8")
    )
      problems.push(`docker/${file} differs from the repository's`);
  } catch (error) {
    problems.push(`cannot check docker/${file}: ${error.message}`);
  }
}
// Developer specimens must not become public routes or links.
if (
  existsSync(join(root, "design-system")) ||
  existsSync(join(root, "design-system.html"))
)
  problems.push(
    "the developer showcase must not be exported as a public route",
  );
let count = 0;
function visit(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const file = join(dir, entry.name);
    if (entry.isDirectory()) {
      visit(file);
      continue;
    }
    const body = readFileSync(file, "utf8");
    if (finalRelease && pendingSigning.test(body))
      problems.push(`${file}: release signing key is still pending`);
    if (/PLACEHOLDER|\.invalid\b/.test(body))
      problems.push(`${file}: unfinished download address`);
    if (!entry.name.endsWith(".html")) continue;
    count++;
    for (const link of body.matchAll(/<(?:a|link)\b[^>]*\bhref="([^"]+)"/g)) {
      const url = new URL(link[1], "https://minanode.org");
      if (
        url.origin === "https://minanode.org" &&
        /^\/design-system(?:\/|$)/.test(url.pathname)
      )
        problems.push(`${file}: link to the developer showcase`);
    }
    const prose = body
      .replace(/<(script|style|pre)\b[^>]*>[\s\S]*?<\/\1>/gi, "")
      .replace(/<!--[\s\S]*?-->/g, "");
    for (const match of prose.matchAll(
      /<\/(?:code|a)>[A-Za-z]|[A-Za-z,]<(?:code|a)\b/g,
    )) {
      problems.push(`${file}: missing inline space near ${match[0]}`);
    }
  }
}
visit(root);
if (
  existsSync(join(root, "sitemap.xml")) &&
  /<loc>https:\/\/minanode\.org\/design-system(?:[\/<])/.test(
    readFileSync(join(root, "sitemap.xml"), "utf8"),
  )
)
  problems.push("sitemap links to the developer showcase");
if (problems.length) {
  console.error(problems.join("\n"));
  process.exitCode = 1;
} else {
  console.log(
    `Content check passed: ${count} HTML files; no inline collisions or unfinished download addresses.`,
  );
}
