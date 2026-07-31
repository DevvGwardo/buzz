# Model-Capability Manifest — Mutation Evidence

**Interpreter coverage**: both generated interpreters are exercised per mutation fault.
- **TypeScript**: `scripts/run-corpus.mjs` imports `resolveModelCapabilities()` from
  `desktop/src/features/agents/ui/modelCapabilities.ts` via `--experimental-strip-types`.
- **Rust**: `cargo test -p buzz-agent -- generated_model_capabilities::tests::shared_corpus_tests`
  deserializes and executes every vector in `scripts/normative-corpus.json` against
  `resolve_model_capabilities()`.

## How to reproduce

```sh
# Runs generator mutations; exercises both TS and Rust interpreters per fault
node --experimental-strip-types scripts/run-mutation-evidence.mjs

# Run interpreters independently:
node --experimental-strip-types scripts/run-corpus.mjs
cargo test -p buzz-agent -- generated_model_capabilities::tests::shared_corpus_tests
```

## Mutation run results (both interpreters)

All 7 mutations applied in isolation; manifest restored after each run.
Each mutation must be detected (killed) by **both** interpreters for it to count as covered.

| ID | Mutation | Expected killer(s) | TS | Rust |
|----|----------|--------------------|----|------|
| M1 | Reduce `claude-opus-4-7` `supported_efforts` to `[low,medium,high]` (drops xhigh+max) | `anthropic-claude-opus-4-7`, `dbv2-claude-prefix-stripped`, `dbv2-claude-route-anthropic-messages` | **killed ✓** | **killed ✓** |
| M2 | Add `xhigh` to `gpt5-base` `supported_efforts` | `openai-gpt5-base`, `openai-gpt5-1106-should-not-match-base`, `openai-gpt5-4o-matches-base`, `openai-gpt5-date-suffix` | **killed ✓** | **killed ✓** |
| M3 | Change `gpt5-1` `default_effort` to `"high"` instead of `"none"` | `openai-gpt5.1` | **killed ✓** | **killed ✓** |
| M4 | Swap `dbv2-claude-code-names-segment` route from `anthropic-messages` to `openai-responses` | `dbv2-goose-opus-5-is-anthropic` | **killed ✓** | **killed ✓** |
| M5 | Remove all three DBv2 segment rules | `dbv2-goose-opus-5-is-anthropic`, `dbv2-consolidated-llama-not-sol`, `dbv2-terraform-coder-not-terra` | **killed ✓** | **killed ✓** |
| M6 | Change `databricks_v2` concrete-unknown fallback route from `mlflow-chat` to `openai-responses` | `dbv2-concrete-unknown-mlflow-no-max` | **killed ✓** | **killed ✓** |
| M7 | Remove `xhigh` from `gpt5-4` `supported_efforts` | `resolver-prefixed-alias-misses-exact` | **killed ✓** | **killed ✓** |

**Summary: 7/7 mutations killed in both TS and Rust interpreters.**

## Coverage gaps

- Provider fallback mutations for `anthropic`, `openai`, `databricks`, `openrouter`, and
  `_default` are not individually mutated. These are covered by explicit fallback vectors
  in the corpus for `anthropic`, `openai`, and `databricks_v2`.
- Rust mutations are run by recompiling the mutated generated file per fault (via `cargo
  test` after `node generate-model-capabilities.mjs`). Compile time is acceptable for
  offline mutation runs; CI only runs the already-compiled shared corpus harness.
