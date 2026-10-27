// node web/run-node.mjs <module.wasm> [filter-regex]
import { readFileSync } from "node:fs";
import { runBench } from "./bench-core.mjs";

const [file, filter] = process.argv.slice(2);
console.log(`== ${file} (node ${process.version}, V8 ${process.versions.v8})`);
const { mismatches } = await runBench(readFileSync(file), {
  filter: filter ? new RegExp(filter) : null,
  log: console.log,
});
process.exit(mismatches === 0 ? 0 : 1);
