# Model-Capability Manifest — Mutation Evidence

**Interpreter coverage**: JS (`scripts/run-corpus.mjs`) and Rust
(`generated_model_capabilities::tests::shared_corpus_tests::test_shared_corpus_all_vectors`).
Both execute every vector in `scripts/normative-corpus.json`. The mutations below were run
against the JS interpreter; the same assertions are exercised by the Rust shared-corpus
harness because both consume the identical vector set.

## How to reproduce

```sh
# JS mutations (runs generator, applies faults, re-runs corpus)
node scripts/run-mutation-evidence.mjs

# Rust shared-corpus harness (verifies Rust interpreter against all 44 vectors)
cargo test -p buzz-agent -- generated_model_capabilities::tests::shared_corpus_tests
```

## Mutation run results (JS interpreter)

All 7 mutations applied in isolation; manifest restored after each. Corpus must fail
(mutation killed) for the mutation to count as detected.

| ID | Mutation | Expected killer(s) | Result |
|----|----------|--------------------|--------|
| M1 | Reduce `claude-opus-4-7` `supported_efforts` to `[low,medium,high]` (drops xhigh+max) | `anthropic-claude-opus-4-7`, `dbv2-claude-prefix-stripped`, `dbv2-claude-route-anthropic-messages` | **killed ✓** |
| M2 | Add `xhigh` to `gpt5-base` `supported_efforts` | `openai-gpt5-base`, `openai-gpt5-1106-should-not-match-base`, `openai-gpt5-4o-matches-base`, `openai-gpt5-date-suffix` | **killed ✓** |
| M3 | Change `gpt5-1` `default_effort` to `"high"` instead of `"none"` | `openai-gpt5.1` | **killed ✓** |
| M4 | Swap `dbv2-claude-code-names-segment` route from `anthropic-messages` to `openai-responses` | `dbv2-goose-opus-5-is-anthropic` | **killed ✓** |
| M5 | Remove all three DBv2 segment rules | `dbv2-goose-opus-5-is-anthropic`, `dbv2-consolidated-llama-not-sol`, `dbv2-terraform-coder-not-terra` | **killed ✓** |
| M6 | Change `databricks_v2` concrete-unknown fallback route from `mlflow-chat` to `openai-responses` | `dbv2-concrete-unknown-mlflow-no-max` | **killed ✓** |
| M7 | Remove `xhigh` from `gpt5-4` `supported_efforts` | `resolver-prefixed-alias-misses-exact` | **killed ✓** |

**Summary: 7/7 mutations killed by the shared corpus.**

## Rust interpreter evidence

The Rust shared-corpus harness (`test_shared_corpus_all_vectors`) deserializes
`scripts/normative-corpus.json` at test time and runs every vector against
`resolve_model_capabilities()`. Any of the above resolver faults introduced into
the generated Rust resolver would cause the same assertions to fail:

- M1/M4/M5: `DatabricksV2Route` or `supported_efforts` assertion fails.
- M2/M7: `supported_efforts` set mismatch detected in corpus vectors.
- M3: `default_effort` assertion fails for `openai-gpt5.1` vector.
- M6: `DatabricksV2Route::MlflowChatCompletions` assertion fails.

The Rust resolver is generated deterministically from the same manifest; any change
that shifts JS behavior identically shifts Rust behavior (same generator, same source).
Cross-interpreter divergences are caught by independent assertion of both interpreters
against the shared corpus.

## Coverage gaps

- Rust mutations are not run per-fault at build time (compilation cost). Regression is
  prevented by: (a) the shared corpus running identically on both interpreters, and
  (b) the regen-diff CI gate ensuring committed artifacts match the manifest.
- Provider fallback mutations for `anthropic`, `openai`, `databricks`, `openrouter`, and
  `_default` are not individually mutated. These are covered by explicit fallback vectors
  in the corpus for `anthropic`, `openai`, and `databricks_v2`.
