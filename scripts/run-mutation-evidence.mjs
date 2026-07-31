#!/usr/bin/env node
/**
 * Per-interpreter mutation evidence runner.
 *
 * Introduces deliberate resolver faults into the manifest, regenerates artifacts,
 * and verifies the shared normative corpus detects every fault in BOTH the generated
 * TypeScript interpreter (via --experimental-strip-types import) and the Rust
 * interpreter (via cargo test shared-corpus harness). Exits 0 if all mutations are
 * killed in both interpreters; exits 1 if any survive.
 *
 * Usage:
 *   node --experimental-strip-types scripts/run-mutation-evidence.mjs [--verbose]
 *
 * This script is non-CI (run manually to generate MUTATION_EVIDENCE.md). It writes
 * its findings to stdout in a format suitable for copy-paste into the evidence doc.
 *
 * Mutations applied (each in isolation, manifest restored after each run):
 *   M1: Swap anthropic-adaptive-xhigh-opus-4-7 efforts from [low,medium,high,xhigh,max]
 *       to [low,medium,high] — kills corpus vectors that check xhigh/max.
 *   M2: Change gpt5-base supported_efforts to include "xhigh" — kills vectors that
 *       check gpt5-base resolves minimal-only, not xhigh.
 *   M3: Change openai-gpt5-1 default_effort to "high" instead of "none" — kills
 *       the gpt5.1 corpus vector that checks default_effort=none.
 *   M4: Swap databricks_v2_wire_route in dbv2-claude-code-names-segment from
 *       "anthropic-messages" to "openai-responses" — kills segment-route corpus vectors.
 *   M5: Remove all three DBv2 segment rules — kills goose-opus-5 and terraform/consolidated
 *       segment collision vectors.
 *   M6: Change databricks_v2 concrete_unknown fallback route from "mlflow-chat" to
 *       "openai-responses" — kills dbv2-concrete-unknown-mlflow-no-max vector.
 *   M7: Change gpt5-4 supported_efforts to remove "xhigh" — kills
 *       resolver-prefixed-alias-misses-exact vector.
 */

import { readFileSync, writeFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { execSync, spawnSync } from "node:child_process";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(__dirname, "..");
const VERBOSE = process.argv.includes("--verbose");

const manifestPath = join(repoRoot, "scripts", "model-capabilities.json");
const generatorPath = join(repoRoot, "scripts", "generate-model-capabilities.mjs");
const jsRunnerPath = join(repoRoot, "scripts", "run-corpus.mjs");

const originalManifest = readFileSync(manifestPath, "utf8");

/**
 * Run the TS corpus and return:
 *   { kind: "passed" }                          — all vectors pass (mutation survived)
 *   { kind: "killed", output }                  — nonzero exit AND at least one expectedKiller ID
 *                                                  appears in stdout/stderr ("FAIL  <id>" line)
 *   { kind: "error", output }                   — nonzero exit but NO expected corpus output
 *                                                  (import error, missing file, syntax error, etc.)
 */
function runTsCorpus(expectedKillers) {
  let result;
  try {
    result = spawnSync(
      process.execPath,
      ["--experimental-strip-types", jsRunnerPath],
      { cwd: repoRoot, encoding: "utf8" },
    );
  } catch (e) {
    return { kind: "error", output: `spawn error: ${e.message}` };
  }
  if (result.status === 0) return { kind: "passed" };
  const output = (result.stdout ?? "") + (result.stderr ?? "");
  // A genuine corpus kill produces "FAIL  <vector-id>" lines.
  // An infrastructure failure (import error, syntax error) produces no such lines.
  const hasCorpusFailure = expectedKillers.some((id) => output.includes(`FAIL  ${id}`));
  if (hasCorpusFailure) return { kind: "killed", output };
  return { kind: "error", output };
}

/**
 * Run the Rust corpus and return:
 *   { kind: "passed" }                          — all vectors pass
 *   { kind: "killed", output }                  — nonzero exit AND at least one expectedKiller ID
 *                                                  appears in the panic output ("[<id>]" format)
 *   { kind: "error", output }                   — nonzero exit but NO expected corpus output
 *                                                  (compile error, missing cargo, linker error, etc.)
 */
function runRustCorpus(expectedKillers) {
  let result;
  try {
    result = spawnSync(
      "cargo",
      [
        "test",
        "-p", "buzz-agent",
        "--",
        "generated_model_capabilities::tests::shared_corpus_tests",
        "--nocapture",
      ],
      { cwd: repoRoot, encoding: "utf8", env: { ...process.env, RUST_BACKTRACE: "0" } },
    );
  } catch (e) {
    return { kind: "error", output: `spawn error: ${e.message}` };
  }
  if (result.status === 0) return { kind: "passed" };
  const output = (result.stdout ?? "") + (result.stderr ?? "");
  // The Rust corpus runner panics with "[<vector-id>] <axis>: got..." messages.
  const hasCorpusFailure = expectedKillers.some((id) => output.includes(`[${id}]`));
  if (hasCorpusFailure) return { kind: "killed", output };
  return { kind: "error", output };
}

function regen() {
  execSync(`"${process.execPath}" "${generatorPath}"`, {
    cwd: repoRoot,
    stdio: VERBOSE ? "inherit" : "pipe",
  });
}

function restore() {
  writeFileSync(manifestPath, originalManifest, "utf8");
}

function applyMutation(mutFn) {
  const manifest = JSON.parse(originalManifest);
  mutFn(manifest);
  writeFileSync(manifestPath, JSON.stringify(manifest, null, 2) + "\n", "utf8");
}

const mutations = [
  {
    id: "M1",
    description: "Reduce claude-opus-4-7 supported_efforts to [low,medium,high] (drops xhigh+max)",
    expectedKillers: ["anthropic-claude-opus-4-7", "dbv2-claude-prefix-stripped", "dbv2-claude-route-anthropic-messages"],
    mutate(manifest) {
      const rule = manifest.family_rules.find(r => r.id === "anthropic-adaptive-xhigh-opus-4-7");
      rule.supported_efforts = ["low", "medium", "high"];
    },
  },
  {
    id: "M2",
    description: "Add xhigh to gpt5-base supported_efforts [minimal,low,medium,high,xhigh]",
    expectedKillers: ["openai-gpt5-base", "openai-gpt5-1106-should-not-match-base", "openai-gpt5-4o-matches-base", "openai-gpt5-date-suffix"],
    mutate(manifest) {
      const rule = manifest.family_rules.find(r => r.id === "openai-gpt5-base");
      rule.supported_efforts = ["minimal", "low", "medium", "high", "xhigh"];
    },
  },
  {
    id: "M3",
    description: "Change gpt5-1 default_effort to 'high' instead of 'none'",
    expectedKillers: ["openai-gpt5.1"],
    mutate(manifest) {
      const rule = manifest.family_rules.find(r => r.id === "openai-gpt5-1");
      rule.default_effort = "high";
    },
  },
  {
    id: "M4",
    description: "Swap dbv2-claude-code-names-segment route from anthropic-messages to openai-responses",
    expectedKillers: ["dbv2-goose-opus-5-is-anthropic"],
    mutate(manifest) {
      const rule = manifest.family_rules.find(r => r.id === "dbv2-claude-code-names-segment");
      rule.databricks_v2_wire_route = "openai-responses";
    },
  },
  {
    id: "M5",
    description: "Remove all three DBv2 segment rules (dbv2-claude-code-names-segment, dbv2-gpt-code-names-segment, dbv2-sol-luna-terra-segment)",
    expectedKillers: ["dbv2-goose-opus-5-is-anthropic", "dbv2-consolidated-llama-not-sol", "dbv2-terraform-coder-not-terra"],
    mutate(manifest) {
      manifest.family_rules = manifest.family_rules.filter(
        r => !["dbv2-claude-code-names-segment", "dbv2-gpt-code-names-segment", "dbv2-sol-luna-terra-segment"].includes(r.id)
      );
    },
  },
  {
    id: "M6",
    description: "Change databricks_v2 concrete_unknown fallback route from mlflow-chat to openai-responses",
    expectedKillers: ["dbv2-concrete-unknown-mlflow-no-max"],
    mutate(manifest) {
      manifest.provider_fallbacks.databricks_v2.concrete_unknown.databricks_v2_wire_route = "openai-responses";
    },
  },
  {
    id: "M7",
    description: "Remove xhigh from gpt5-4 supported_efforts [none,low,medium,high]",
    expectedKillers: ["resolver-prefixed-alias-misses-exact"],
    mutate(manifest) {
      const rule = manifest.family_rules.find(r => r.id === "openai-gpt5-4");
      rule.supported_efforts = ["none", "low", "medium", "high"];
    },
  },
];

let allKilled = true;
const results = [];

for (const mut of mutations) {
  process.stdout.write(`  ${mut.id}: ${mut.description}\n`);
  try {
    applyMutation(mut.mutate);
    regen();

    // TS interpreter
    process.stdout.write(`    TS  ... `);
    const tsResult = runTsCorpus(mut.expectedKillers);
    const tsKilled = tsResult.kind === "killed";
    const tsError = tsResult.kind === "error";
    if (tsKilled) {
      process.stdout.write("killed ✓\n");
    } else if (tsError) {
      process.stdout.write(`ERROR (infrastructure failure — not a corpus kill)\n`);
      if (VERBOSE) process.stdout.write(`      ${tsResult.output}\n`);
    } else {
      process.stdout.write("SURVIVED ✗\n");
      if (VERBOSE) process.stdout.write(`      ${tsResult.output ?? ""}\n`);
    }

    // Rust interpreter
    process.stdout.write(`    Rust... `);
    const rustResult = runRustCorpus(mut.expectedKillers);
    const rustKilled = rustResult.kind === "killed";
    const rustError = rustResult.kind === "error";
    if (rustKilled) {
      process.stdout.write("killed ✓\n");
    } else if (rustError) {
      process.stdout.write(`ERROR (infrastructure failure — not a corpus kill)\n`);
      if (VERBOSE) process.stdout.write(`      ${rustResult.output}\n`);
    } else {
      process.stdout.write("SURVIVED ✗\n");
      if (VERBOSE) process.stdout.write(`      ${rustResult.output ?? ""}\n`);
    }

    const killed = tsKilled && rustKilled;
    if (!killed) allKilled = false;
    results.push({ ...mut, killed, tsKilled, rustKilled, tsError, rustError });
  } catch (e) {
    process.stdout.write(`    ERROR: ${e.message}\n`);
    allKilled = false;
    results.push({ ...mut, killed: false, tsKilled: false, rustKilled: false, tsError: true, rustError: true, output: e.message });
  } finally {
    restore();
    regen(); // restore generated files
  }
}

console.log("");
const killed = results.filter(r => r.killed).length;
const errored = results.filter(r => r.tsError || r.rustError).length;
console.log(`Mutation results: ${killed}/${results.length} killed (both interpreters)` +
  (errored > 0 ? `, ${errored} ERROR (infrastructure failure — see output above)` : ""));

if (!allKilled) {
  const hasErrors = results.some(r => r.tsError || r.rustError);
  if (hasErrors) {
    console.error("ERROR: Infrastructure failures prevented some mutations from being verified as killed.");
    console.error("       Run with --verbose to see the full output for ERROR entries.");
  }
  console.error("ERROR: Some mutations survived — corpus does not kill all resolver faults.");
  process.exit(1);
}

console.log("All mutations killed in both TS and Rust interpreters.");
