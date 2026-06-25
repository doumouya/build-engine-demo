#!/usr/bin/env node
/* scrub-audit — enforces the "no leaked source-project identity" rule.
 *
 * This repo is built by generalizing patterns from a private source project; nothing branded from
 * that source may leak into the public tree (names, comments, filenames, CSS prefixes). This audit
 * scans tracked text files for forbidden identity tokens and exits with the violation count (the
 * ci-audit ratchet reads the exit code; baseline = 0, so any leak fails CI).
 *
 * The forbidden tokens are assembled at runtime so THIS file never contains the literal strings
 * (it would otherwise match itself). It also excludes its own dir + the baseline. */

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

// assembled so this source file does not self-match
const FORBIDDEN = [["red", "pash"].join(""), ["rp", "-"].join("")];
const RE = new RegExp(FORBIDDEN.join("|"), "i");

const SKIP = /(^|\/)(\.git\/|node_modules\/|tools\/scrub-audit\/|tools\/ci-audit\/baseline\.json$)/;
const TEXT = /\.(js|mjs|ts|rs|css|html?|md|json|sql|sh|toml|txt|ya?ml)$/i;

let files = [];
try {
  files = execFileSync("git", ["ls-files"], { cwd: ROOT, encoding: "utf8" })
    .split("\n").filter(Boolean);
} catch {
  /* not a git repo yet — nothing tracked to scan */
}

const hits = [];
for (const f of files) {
  if (SKIP.test(f) || !TEXT.test(f)) continue;
  let body;
  try { body = readFileSync(join(ROOT, f), "utf8"); } catch { continue; }
  body.split("\n").forEach((ln, i) => {
    if (RE.test(ln)) hits.push(`${f}:${i + 1}: ${ln.trim().slice(0, 100)}`);
  });
}

if (hits.length) {
  console.error(`scrub-audit: ${hits.length} forbidden-token line(s) — clean before publishing:`);
  for (const h of hits) console.error("  " + h);
} else {
  console.log("scrub-audit: clean (no forbidden identity tokens).");
}
process.exit(hits.length);
