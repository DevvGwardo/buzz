# models.dev Reasoning Options Reconciliation Table

**Source queried**: https://models.dev (2025-07-30)  
**Policy (plan v4 §Behavior policy)**: models.dev `reasoning_options` become exact overrides.
Each divergence from the current family rule result is reconciled here: either (a) adopted as an
intentional correction or (b) rejected with a curation note.

**Pinned source artifact**: `scripts/catalog-sample-fixture.json` — sanitized snapshot of the
catalog used during reconciliation. Re-query: `python3 scripts/generate-databricks-model-names.py`.

## Divergences

### `databricks-gpt-5-4-mini`

| | Current family rule (gpt5-4) | models.dev | Disposition |
|---|---|---|---|
| `supported_efforts` | `[none, low, medium, high, xhigh]` | `[low, medium, high]` | **ADOPT** |

**Rationale**: The Databricks AI Gateway v2 endpoint for `databricks-gpt-5-4-mini` explicitly
advertises only `[low, medium, high]` in its `reasoning_options`. The family rule's `none` and
`xhigh` are derived from the upstream OpenAI GPT-5.4 spec, which this Databricks endpoint does
not expose. Provider-advertised wins per plan F1 policy.

**Source**: [https://models.dev](https://models.dev) — queried 2025-07-30; `providers.databricks.models["databricks-gpt-5-4-mini"].reasoning_options = ["low","medium","high"]`  
**Snapshot**: `scripts/catalog-sample-fixture.json` key `"databricks-gpt-5-4-mini"`  
**Test vector**: `resolver-exact-raw-id-hit` in `scripts/normative-corpus.json`

---

### `databricks-gpt-5-4-nano`

| | Current family rule (gpt5-4) | models.dev | Disposition |
|---|---|---|---|
| `supported_efforts` | `[none, low, medium, high, xhigh]` | `[low, medium, high]` | **ADOPT** |

**Rationale**: Same as `databricks-gpt-5-4-mini`. The nano variant exposes the same restricted
effort set. Provider-advertised wins.

**Source**: [https://models.dev](https://models.dev) — queried 2025-07-30; `providers.databricks.models["databricks-gpt-5-4-nano"].reasoning_options = ["low","medium","high"]`  
**Snapshot**: `scripts/catalog-sample-fixture.json` key `"databricks-gpt-5-4-nano"`

---

### `databricks-gpt-5-6-sol`

| | Current family rule (gpt5-6) | models.dev | Disposition |
|---|---|---|---|
| `supported_efforts` | `[none, low, medium, high, xhigh, max]` | `[none, low, medium, high, xhigh, max]` | **NO DIVERGENCE** |

**Rationale**: Consistent with family rule. No override needed. Exact record included only for
catalog-completeness (it's a `DATABRICKS_V2_KNOWN_MODELS` candidate).

---

### `databricks-gpt-5-5`

| | Current family rule (gpt5-5) | models.dev | Disposition |
|---|---|---|---|
| `supported_efforts` | `[none, low, medium, high, xhigh]` | Not queried (not in models.dev at 2025-07-30) | **NO DIVERGENCE** |

**Rationale**: `DATABRICKS_V2_KNOWN_MODELS` entry. Family rule applies. No models.dev data
available to diverge from.

---

### `databricks-claude-opus-4-7`

| | Current family rule (anthropic-adaptive-xhigh-opus-4-7) | models.dev | Disposition |
|---|---|---|---|
| `supported_efforts` | `[low, medium, high, xhigh, max]` | Not in models.dev (Anthropic model) | **NO DIVERGENCE** |

**Rationale**: `DATABRICKS_V2_KNOWN_MODELS` entry. Anthropic family rule applies. No models.dev
`reasoning_options` data to conflict with.

---

## Non-divergences (confirmed consistent)

The following models were checked against models.dev or provider docs and found consistent with
the manifest family rules. No exact records needed.

| Model family | Source | Checked against | Status |
|---|---|---|---|
| `claude-opus-4-7` | [https://platform.claude.com/docs/en/build-with-claude/extended-thinking](https://platform.claude.com/docs/en/build-with-claude/extended-thinking) | Anthropic extended-thinking support table (July 2025) | ✓ Consistent |
| `claude-opus-4-8` | [https://platform.claude.com/docs/en/build-with-claude/extended-thinking](https://platform.claude.com/docs/en/build-with-claude/extended-thinking) | Anthropic extended-thinking support table (July 2025) | ✓ Consistent |
| `claude-sonnet-5.*` | [https://platform.claude.com/docs/en/build-with-claude/extended-thinking](https://platform.claude.com/docs/en/build-with-claude/extended-thinking) | Anthropic extended-thinking support table (July 2025) | ✓ Consistent |
| `claude-fable-5` | [https://platform.claude.com/docs/en/build-with-claude/extended-thinking](https://platform.claude.com/docs/en/build-with-claude/extended-thinking) | Anthropic extended-thinking support table (July 2025) | ✓ Consistent |
| `claude-mythos-5` | [https://platform.claude.com/docs/en/build-with-claude/extended-thinking](https://platform.claude.com/docs/en/build-with-claude/extended-thinking) | Anthropic extended-thinking support table (July 2025) | ✓ Consistent |
| `claude-opus-4-6` | [https://platform.claude.com/docs/en/build-with-claude/extended-thinking](https://platform.claude.com/docs/en/build-with-claude/extended-thinking) | Anthropic extended-thinking support table (July 2025) | ✓ Consistent |
| `claude-sonnet-4-6` | [https://platform.claude.com/docs/en/build-with-claude/extended-thinking](https://platform.claude.com/docs/en/build-with-claude/extended-thinking) | Anthropic extended-thinking support table (July 2025) | ✓ Consistent |
| `claude-mythos-preview` | [https://platform.claude.com/docs/en/build-with-claude/extended-thinking](https://platform.claude.com/docs/en/build-with-claude/extended-thinking) | Anthropic extended-thinking support table (July 2025) | ✓ Consistent |
| `claude-3*` | [https://platform.claude.com/docs/en/build-with-claude/extended-thinking](https://platform.claude.com/docs/en/build-with-claude/extended-thinking) | Anthropic extended-thinking support table (July 2025) | ✓ Consistent |
| `gpt-5-pro` | [https://platform.openai.com/docs/guides/reasoning](https://platform.openai.com/docs/guides/reasoning) | OpenAI reasoning guide (July 2025) | ✓ Consistent |
| `gpt-5.6` | [https://platform.openai.com/docs/guides/reasoning](https://platform.openai.com/docs/guides/reasoning) | OpenAI reasoning guide (July 2025) | ✓ Consistent |
| `gpt-5.5` | [https://platform.openai.com/docs/guides/reasoning](https://platform.openai.com/docs/guides/reasoning) | OpenAI reasoning guide (July 2025) | ✓ Consistent |
| `gpt-5.4` | [https://platform.openai.com/docs/guides/reasoning](https://platform.openai.com/docs/guides/reasoning) | OpenAI reasoning guide (July 2025) | ✓ Consistent |
| `gpt-5.1` | [https://platform.openai.com/docs/guides/reasoning](https://platform.openai.com/docs/guides/reasoning) | OpenAI reasoning guide (July 2025) | ✓ Consistent |
| `gpt-5` (base) | [https://platform.openai.com/docs/guides/reasoning](https://platform.openai.com/docs/guides/reasoning) | OpenAI reasoning guide (July 2025) | ✓ Consistent |
