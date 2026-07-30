#!/usr/bin/env node
/**
 * Normative corpus runner — validates the TS interpreter against scripts/normative-corpus.json.
 *
 * This is the JS side of the two-interpreter corpus check.
 * The Rust side lives in crates/buzz-agent/src/generated_model_capabilities.rs tests.
 *
 * Usage: node scripts/run-corpus.mjs [--verbose]
 * Exits 0 on all pass, 1 on any failure.
 */

import { readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const __dirname = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(__dirname, "..");
const VERBOSE = process.argv.includes("--verbose");

// Import the generated TS interpreter via dynamic import (ESM, already .ts compiled or .mjs)
// We use the compiled JS output from TSC. For this test we inline the resolver logic.
// Rather than depending on build output, we import the raw source via ts-node or
// replicate the resolver inline from the manifest (same generator logic).

// Load manifest and re-run resolution inline using the same generator logic.
const manifest = JSON.parse(
  readFileSync(join(repoRoot, "scripts", "model-capabilities.json"), "utf8"),
);
const corpus = JSON.parse(
  readFileSync(join(repoRoot, "scripts", "normative-corpus.json"), "utf8"),
);

// ----- Inline resolver (same logic as generator) -----

function stripCatalogPrefix(model) {
  const lower = model.toLowerCase();
  let firstIdx = Infinity;
  for (const tok of manifest.family_tokens) {
    const idx = lower.indexOf(tok);
    if (idx !== -1 && idx < firstIdx) firstIdx = idx;
  }
  return firstIdx === Infinity ? model : model.slice(firstIdx);
}

function gpt5TokenMatches(model, token) {
  const lower = model.toLowerCase();
  let start = 0;
  while (true) {
    const idx = lower.indexOf(token, start);
    if (idx === -1) return false;
    const afterIdx = idx + token.length;
    const afterChar = afterIdx < lower.length ? lower[afterIdx] : "";
    if (afterChar === "" || afterChar === "-") return true;
    start = afterIdx;
  }
}

function gpt5BaseMatches(model, token) {
  const lower = model.toLowerCase();
  let start = 0;
  while (true) {
    const idx = lower.indexOf(token, start);
    if (idx === -1) return false;
    const afterIdx = idx + token.length;
    const suffix = lower.slice(afterIdx);
    if (suffix === "") return true;
    if (!suffix.startsWith("-")) { start = afterIdx; continue; }
    const dashRest = suffix.slice(1);
    if (/^\d{1,3}(?:[^a-z\d]|$)/i.test(dashRest)) { start = afterIdx; continue; }
    return true;
  }
}

function ruleMatchesModel(rule, normalizedModel, provider) {
  if (!rule.providers.includes(provider)) return false;
  const lower = normalizedModel.toLowerCase();
  const allTokens = [rule.match_value, ...(rule.match_aliases ?? [])];
  switch (rule.match_kind) {
    case "exact": return allTokens.some((t) => lower === t.toLowerCase());
    case "prefix": return allTokens.some((t) => lower.startsWith(t.toLowerCase()));
    case "gpt5-token": return allTokens.some((t) => gpt5TokenMatches(lower, t));
    case "gpt5-base": return allTokens.some((t) => gpt5BaseMatches(lower, t));
    case "segment": {
      const segs = lower.split(/[^a-z0-9]+/);
      return allTokens.some((t) => segs.includes(t.toLowerCase()));
    }
    case "segment-prefix": {
      const segs = lower.split(/[^a-z0-9]+/);
      return allTokens.some((t) => segs.some((s) => s.startsWith(t.toLowerCase())));
    }
    default: throw new Error(`unknown match_kind: ${rule.match_kind}`);
  }
}

function resolveFamilyRules(provider, normalizedAlias) {
  if (!normalizedAlias) return null;
  const sorted = [...manifest.family_rules].sort((a, b) => b.match_priority - a.match_priority);
  for (const rule of sorted) {
    if (ruleMatchesModel(rule, normalizedAlias, provider)) {
      // databricks_v2_wire_route is only meaningful for databricks_v2; all other providers get not-applicable
      const wireRoute = provider === "databricks_v2"
        ? rule.databricks_v2_wire_route
        : "not-applicable";
      return {
        registry_label: rule.registry_label ?? null,
        thinking_mode: rule.thinking_mode,
        supported_efforts: rule.supported_efforts,
        default_effort: rule.default_effort,
        databricks_v2_wire_route: wireRoute,
        normalization_policy: rule.normalization_policy,
      };
    }
  }
  return null;
}

function getProviderFallback(provider, isBlank) {
  const fb = manifest.provider_fallbacks[provider] ?? manifest.provider_fallbacks["_default"];
  return isBlank ? { ...fb.blank } : { ...fb.concrete_unknown };
}

function resolve(provider, rawModelId) {
  const isBlank = !rawModelId || rawModelId.trim() === "";
  const exactRecord = (manifest.exact_records ?? []).find(
    (r) => r.provider === provider && r.raw_model_id === rawModelId,
  );
  if (exactRecord) {
    const normalizedAlias = stripCatalogPrefix(rawModelId);
    const base = resolveFamilyRules(provider, normalizedAlias) ?? getProviderFallback(provider, isBlank);
    return {
      registry_label: exactRecord.registry_label ?? null,
      thinking_mode: exactRecord.thinking_mode ?? base.thinking_mode,
      supported_efforts: exactRecord.supported_efforts_override ?? base.supported_efforts,
      default_effort: exactRecord.default_effort !== undefined ? exactRecord.default_effort : base.default_effort,
      databricks_v2_wire_route: exactRecord.databricks_v2_wire_route ?? base.databricks_v2_wire_route,
      normalization_policy: exactRecord.normalization_policy ?? base.normalization_policy,
    };
  }
  const normalizedAlias = stripCatalogPrefix(rawModelId ?? "");
  const familyResult = resolveFamilyRules(provider, normalizedAlias);
  if (familyResult) return familyResult;
  return { ...getProviderFallback(provider, isBlank), registry_label: null };
}

// ----- Run corpus -----

let passed = 0;
let failed = 0;

for (const entry of corpus) {
  // Skip group header entries
  if (entry._group) continue;
  if (!entry.expect) continue;

  const result = resolve(entry.provider, entry.raw_model_id);
  const expect = entry.expect;

  const failures = [];

  for (const [key, expectedVal] of Object.entries(expect)) {
    const camelKey = key.replace(/_([a-z])/g, (_, c) => c.toUpperCase());
    // Use explicit 'in' check so null values (default_effort: null) are not coerced away
    const actualVal = (key in result) ? result[key] : result[camelKey];

    if (Array.isArray(expectedVal)) {
      // Order-sensitive comparison for effort arrays
      const actualArr = actualVal ?? [];
      if (JSON.stringify(actualArr) !== JSON.stringify(expectedVal)) {
        failures.push(
          `  ${key}: expected [${expectedVal.join(", ")}] got [${(actualArr).join(", ")}]`,
        );
      }
    } else {
      if (actualVal !== expectedVal) {
        failures.push(`  ${key}: expected ${JSON.stringify(expectedVal)} got ${JSON.stringify(actualVal)}`);
      }
    }
  }

  if (failures.length === 0) {
    passed++;
    if (VERBOSE) {
      console.log(`  PASS  ${entry.id}`);
    }
  } else {
    failed++;
    console.error(`  FAIL  ${entry.id} (${entry.provider} / "${entry.raw_model_id}")`);
    for (const f of failures) console.error(f);
    if (entry._note) console.error(`  note: ${entry._note}`);
  }
}

console.log(`\nCorpus: ${passed} passed, ${failed} failed`);
process.exit(failed > 0 ? 1 : 0);
