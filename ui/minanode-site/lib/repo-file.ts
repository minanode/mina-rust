// Files of the repository that a page shows as they are, read at build time
// from the checkout the site is built in (ui/minanode-site is two levels
// down), so the page can't drift from them.
import { readFileSync } from "node:fs";
import { join } from "node:path";

export function readRepoFile(path: string) {
  return readFileSync(join(process.cwd(), "../..", path), "utf8");
}
