#!/usr/bin/env node
/* agent-refs-audit — keeps the agent chain's own references honest.
 *
 * The build-engine's whole credibility is "every app was built through the agent chain," so the
 * chain's prompts must not point at commands, scripts, or docs that don't exist. This audit scans
 * the orchestrator prompts for referenced repo paths and fails the ratchet for each that doesn't
 * resolve — so a renamed gate or moved doc fails the TOOL, not a live agent run. Exit = the
 * unresolved-reference count (the ci-audit ratchet reads it; baseline 0).
 *
 * It only treats backticked tokens that look like repo paths as references. MCP tool names
 * (case_*), shell verbs, and generic prose are not paths. Runtime-written paths (docs/specs|state|
 * cases/<file>) and patterns/placeholders (`*`, `<...>`) are resolved against their nearest concrete
 * ancestor directory, not the not-yet-created file. */

import { readFileSync, existsSync } from "node:fs";
import { resolve, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const SOURCES = [
  ".claude/agents/architect.md", ".claude/agents/tester.md", ".claude/agents/coder.md",
  ".claude/agents/reviewer.md", ".claude/agents/ops.md", ".claude/commands/feature.md",
  ".claude/case-first-reminder.txt", "CLAUDE.md",
];

const BACKTICK = /`([^`]+)`/g;
const EXT = /\.(sh|mjs|js|json|md|sql|toml|txt|ya?ml)$/;
const RUNTIME = /^docs\/(specs|state|cases)\//; // runtime-written files: check the dir, not the file

function isPathLike(t) {
  return /^(tools\/|docs\/|\.claude\/)/.test(t) || (EXT.test(t) && !t.includes(" "));
}
function concreteAncestorExists(p) {
  const out = [];
  for (const seg of p.split("/")) { if (/[*<>]/.test(seg)) break; out.push(seg); }
  const dir = out.join("/");
  return dir === "" ? true : existsSync(join(ROOT, dir));
}
function resolves(p) {
  if (/[*<>]/.test(p)) return concreteAncestorExists(p);  // pattern / placeholder
  if (RUNTIME.test(p)) return existsSync(join(ROOT, dirname(p))); // runtime-written file
  return existsSync(join(ROOT, p));
}

const unresolved = [];
for (const f of SOURCES) {
  const abs = join(ROOT, f);
  if (!existsSync(abs)) { unresolved.push(`(missing source) ${f}`); continue; }
  const body = readFileSync(abs, "utf8");
  const seen = new Set();
  let m;
  while ((m = BACKTICK.exec(body))) {
    for (const raw of m[1].trim().split(/\s+/)) {
      const tok = raw.replace(/[.,;:)]+$/, ""); // trim trailing punctuation
      if (!isPathLike(tok) || seen.has(tok)) continue;
      seen.add(tok);
      if (!resolves(tok)) unresolved.push(`${f}: ${tok}`);
    }
  }
}

if (unresolved.length) {
  console.error(`agent-refs-audit: ${unresolved.length} unresolved reference(s) in the agent prompts:`);
  for (const u of unresolved) console.error("  " + u);
} else {
  console.log("agent-refs-audit: clean (every path the agent prompts reference resolves).");
}
process.exit(unresolved.length);
