# models.dev Reasoning Options Reconciliation Table

**Source queried**: https://models.dev/api.json (2026-07-31)  
**Payload SHA-256**: `d5a4974cd69f19b0f67713acaa6bb3b16e920defdc07ecbdf6b0a936181bb0e0`  
**Policy (plan v4 §Behavior policy)**: models.dev `reasoning_options` become exact overrides.
Each divergence from the current family rule result is reconciled here: either (a) adopted as an
intentional correction or (b) rejected with a curation note.

**Verbatim source snapshot**: `scripts/catalog-sample-fixture.json` — verbatim `id`, `name`, and
nested `reasoning_options` objects captured from the live API without transformation.
Re-verify hash: `curl -s https://models.dev/api.json | sha256sum`

## Divergences

### `databricks-gpt-5-4-mini`

| | Current family rule (gpt5-4) | models.dev | Disposition |
|---|---|---|---|
| `supported_efforts` | `[none, low, medium, high, xhigh]` | `[low, medium, high]` | **ADOPT** |

**Rationale**: The Databricks AI Gateway v2 endpoint for `databricks-gpt-5-4-mini` explicitly
advertises only `[low, medium, high]` in its `reasoning_options`. The family rule's `none` and
`xhigh` are derived from the upstream OpenAI GPT-5.4 spec, which this Databricks endpoint does
not expose. Provider-advertised wins per plan F1 policy.

**Source**: [https://models.dev/api.json](https://models.dev/api.json) — retrieved 2026-07-31; `providers.databricks.models["databricks-gpt-5-4-mini"].reasoning_options = [{"type":"effort","values":["low","medium","high"]}]`  
**Snapshot**: `scripts/catalog-sample-fixture.json` key `"databricks-gpt-5-4-mini"`  
**Test vector**: `resolver-exact-raw-id-hit` in `scripts/normative-corpus.json`

---

### `databricks-gpt-5-4-nano`

| | Current family rule (gpt5-4) | models.dev | Disposition |
|---|---|---|---|
| `supported_efforts` | `[none, low, medium, high, xhigh]` | `[low, medium, high]` | **ADOPT** |

**Rationale**: Same as `databricks-gpt-5-4-mini`. The nano variant exposes the same restricted
effort set. Provider-advertised wins.

**Source**: [https://models.dev/api.json](https://models.dev/api.json) — retrieved 2026-07-31; `providers.databricks.models["databricks-gpt-5-4-nano"].reasoning_options = [{"type":"effort","values":["low","medium","high"]}]`  
**Snapshot**: `scripts/catalog-sample-fixture.json` key `"databricks-gpt-5-4-nano"`

---

### `databricks-gpt-5-6-sol`

| | Current family rule (gpt5-6) | models.dev | Disposition |
|---|---|---|---|
| `supported_efforts` | `[none, low, medium, high, xhigh, max]` | `[low, medium, high, max]` | **ADOPT** |

**Rationale**: The Databricks AI Gateway v2 endpoint for `databricks-gpt-5-6-sol` advertises only
`[low, medium, high, max]` in its `reasoning_options`. The family rule's `none` and `xhigh` are
derived from the upstream OpenAI GPT-5.6 spec, which this Databricks endpoint does not expose.
Provider-advertised wins per plan F1 policy.

**Source**: [https://models.dev/api.json](https://models.dev/api.json) — retrieved 2026-07-31; `providers.databricks.models["databricks-gpt-5-6-sol"].reasoning_options = [{"type":"effort","values":["low","medium","high","max"]}]`  
**Snapshot**: `scripts/catalog-sample-fixture.json` key `"databricks-gpt-5-6-sol"`

---

### `databricks-gpt-5-5`

| | Current family rule (gpt5-5) | models.dev | Disposition |
|---|---|---|---|
| `supported_efforts` | `[none, low, medium, high, xhigh]` | `[low, medium, high]` | **ADOPT** |

**Rationale**: The Databricks AI Gateway v2 endpoint for `databricks-gpt-5-5` advertises only
`[low, medium, high]` in its `reasoning_options`. The family rule's `none` and `xhigh` are
derived from the upstream OpenAI GPT-5.5 spec, which this Databricks endpoint does not expose.
Provider-advertised wins per plan F1 policy.

**Source**: [https://models.dev/api.json](https://models.dev/api.json) — retrieved 2026-07-31; `providers.databricks.models["databricks-gpt-5-5"].reasoning_options = [{"type":"effort","values":["low","medium","high"]}]`  
**Snapshot**: `scripts/catalog-sample-fixture.json` key `"databricks-gpt-5-5"`

---

### `databricks-claude-opus-4-7`

| | Current family rule (anthropic-adaptive-xhigh-opus-4-7) | models.dev | Disposition |
|---|---|---|---|
| `reasoning_options` type | effort-based | `budget_tokens` | **NO EFFORT DIVERGENCE** |

**Rationale**: models.dev advertises `reasoning_options=[{"type":"budget_tokens","min":1024}]` —
a different capability axis (extended thinking token budget), not an effort-level selector.
There is no effort divergence to reconcile. The effort capabilities for this model come from the
`anthropic-adaptive-xhigh-opus-4-7` family rule (Anthropic extended-thinking support table).

**Source**: [https://models.dev/api.json](https://models.dev/api.json) — retrieved 2026-07-31; `providers.databricks.models["databricks-claude-opus-4-7"].reasoning_options = [{"type":"budget_tokens","min":1024}]`  
**Snapshot**: `scripts/catalog-sample-fixture.json` key `"databricks-claude-opus-4-7"`

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
