#!/usr/bin/env node
/* todo-audit — counts deferred-work markers (TODO / FIXME / XXX / HACK) in tracked source, so the
 * backlog can't quietly grow. Exits with the violation count (the ci-audit ratchet reads the exit
 * code and fails only on NEW markers vs the committed baseline). */

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const RE = /\b(TODO|FIXME|XXX|HACK)\b/;
const SKIP = /(^|\/)(\.git\/|node_modules\/|tools\/todo-audit\/audit\.js$)/;
const TEXT = /\.(js|mjs|ts|rs|css|html?|sql|sh|toml|ya?ml)$/i;

let files = [];
try {
  files = execFileSync("git", ["ls-files"], { cwd: ROOT, encoding: "utf8" })
    .split("\n").filter(Boolean);
} catch { /* not a git repo yet */ }

const hits = [];
for (const f of files) {
  if (SKIP.test(f) || !TEXT.test(f)) continue;
  let body;
  try { body = readFileSync(join(ROOT, f), "utf8"); } catch { continue; }
  body.split("\n").forEach((ln, i) => {
    if (RE.test(ln)) hits.push(`${f}:${i + 1}`);
  });
}

if (hits.length) {
  console.error(`todo-audit: ${hits.length} deferred-work marker(s):`);
  for (const h of hits) console.error("  " + h);
} else {
  console.log("todo-audit: clean (no TODO/FIXME/XXX/HACK markers).");
}
process.exit(hits.length);
