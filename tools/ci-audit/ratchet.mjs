#!/usr/bin/env node
/* ratchet.mjs — the file-based regression gate for tools/ci-audit/check.sh.
 *
 * Computes a violation count per tools/*-audit/audit.js, compares it to the committed baseline
 * (tools/ci-audit/baseline.json), and:
 *   - exit 0 when every tool is at or below baseline (fixed / improved / unchanged are wins);
 *   - exit 1 with a markdown table when any tool is ABOVE baseline (a regression) or appears with
 *     violations but has no baseline entry yet;
 *   - mode `update` rewrites baseline.json from the current counts (exit 0).
 *
 * Counting a tool's violations — two audit shapes are supported:
 *   - emits audit.json (an array of findings, or {violations|findings|results: [...]} or {count})
 *     → count read from disk (works in --no-run);
 *   - prints to stderr + exits with the count (e.g. the scrub/todo audits)
 *     → count = the process exit code, captured by re-running the audit. */

import { readFileSync, existsSync, writeFileSync, readdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { resolve, dirname, basename, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..", "..");
const TOOLS = join(ROOT, "tools");
const BASELINE_PATH = join(HERE, "baseline.json");
const mode = process.argv[2] || "run"; // run | no-run | update

// discover tools/*-audit/audit.js, gated on git-tracked so the baseline stays reproducible.
function isTracked(file) {
  try {
    execFileSync("git", ["ls-files", "--error-unmatch", relative(ROOT, file)], { cwd: ROOT, stdio: "ignore" });
    return true;
  } catch { return false; }
}
function discoverAudits() {
  const out = [];
  for (const ent of readdirSync(TOOLS, { withFileTypes: true })) {
    if (!ent.isDirectory() || !ent.name.endsWith("-audit")) continue;
    const dir = join(TOOLS, ent.name);
    const audit = join(dir, "audit.js");
    if (existsSync(audit) && isTracked(audit)) out.push({ tool: basename(ent.name, "-audit"), dir });
  }
  return out.sort((a, b) => a.tool.localeCompare(b.tool));
}

function countFor({ dir }) {
  const jsonPath = join(dir, "audit.json");
  if (existsSync(jsonPath)) {
    try {
      const data = JSON.parse(readFileSync(jsonPath, "utf8"));
      const arr = Array.isArray(data) ? data : data.violations ?? data.findings ?? data.results ?? null;
      if (Array.isArray(arr)) return arr.length;
      if (typeof data.count === "number") return data.count;
    } catch { /* malformed → fall through to exit-code probe */ }
  }
  try {
    execFileSync("node", [join(dir, "audit.js")], { stdio: "ignore" });
    return 0;
  } catch (e) {
    if (typeof e.status === "number") return e.status;
    throw e; // a real spawn failure, not a violation count
  }
}

const audits = discoverAudits();
const current = {};
for (const a of audits) current[a.tool] = countFor(a);

if (mode === "update") {
  writeFileSync(BASELINE_PATH, JSON.stringify(current, null, 2) + "\n");
  console.log(`  ✓ baseline updated (${BASELINE_PATH}):`);
  for (const [t, c] of Object.entries(current)) console.log(`      ${t}: ${c}`);
  process.exit(0);
}

let baseline = {};
if (existsSync(BASELINE_PATH)) {
  try { baseline = JSON.parse(readFileSync(BASELINE_PATH, "utf8")); }
  catch { console.error("ci-audit: baseline.json is malformed — run --update-baseline."); process.exit(2); }
} else {
  console.error("ci-audit: no baseline.json. Establish one with:\n    sh tools/ci-audit/check.sh --update-baseline");
  process.exit(2);
}

const regressions = [];
for (const [tool, cur] of Object.entries(current)) {
  const prev = baseline[tool];
  if (prev === undefined) { if (cur > 0) regressions.push({ tool, status: "new", prev: "—", cur }); }
  else if (cur > prev) regressions.push({ tool, status: "regressed", prev, cur });
}

if (regressions.length === 0) {
  console.log("  ✓ no new or regressed findings across audited tools.");
  process.exit(0);
}

console.log("  ✗ regressions detected — see the table below.\n");
console.log("## Audit regressions\n");
console.log("Latest run vs the committed baseline (tools/ci-audit/baseline.json). `regressed` = more violations than baseline; `new` = a tool with violations and no baseline entry. After an intentional change, accept the new counts with `sh tools/ci-audit/check.sh --update-baseline`.\n");
console.log("| Tool | Status | Violations (baseline → current) |");
console.log("|---|---|---|");
for (const r of regressions) console.log(`| ${r.tool} | ${r.status} | ${r.prev} → ${r.cur} |`);
console.log("\nDrill into a finding via the tool itself: node tools/<tool>-audit/audit.js");
process.exit(1);
