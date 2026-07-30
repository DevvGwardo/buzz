# Model Capabilities Manifest — Schema Reference

**Source of truth**: `scripts/model-capabilities.json`  
**Generator**: `scripts/generate-model-capabilities.mjs`  
**Emitted artifacts**:
- `crates/buzz-agent/src/generated_model_capabilities.rs`
- `desktop/src/features/agents/ui/modelCapabilities.ts`
- `scripts/generated-model-capabilities-coverage.json` (test fixture)

## Resolver contract (plan v4)

Resolution is a total function `resolve(provider, raw_model_id) → CapabilityResult`.
Three ordered steps:

1. **Provider-qualified raw exact lookup** — key is `(provider, raw_model_id)`, matched on the
   RAW ID **before any prefix stripping**. A prefixed alias never inherits an exact record.
2. **Provider-scoped ordered family rules** — on the normalized (prefix-stripped) alias.
   Rules ordered by `match_priority` descending (higher wins). Each rule is tagged with the
   providers it applies to.
3. **Per-axis provider fallback** — `blank` (empty model string) vs `concrete_unknown`
   (nonblank but unmatched), per provider.

`CapabilityResult` is complete — every axis is populated. Runtime consumers never compose fields
from multiple tiers.

## Axes (schema fields)

| Axis | Type | Notes |
|------|------|-------|
| `registry_label` | `string \| null` | Optional static display label. Feeds `resolveModelLabel()` registry tier only. |
| `thinking_mode` | enum | `manual-budget \| adaptive \| omit-fields \| none \| not-applicable` |
| `supported_efforts` | `ThinkingEffort[]` | Non-empty. UI effort dropdown options. |
| `default_effort` | `ThinkingEffort \| null` | null = "Inherit" (Anthropic manual-budget models). |
| `databricks_v2_wire_route` | enum | `openai-responses \| anthropic-messages \| mlflow-chat \| route-unknown \| not-applicable` |
| `normalization_policy` | enum | `none \| openai-standard \| openai-clamp-max-to-xhigh` |

### `thinking_mode` values

| Value | Meaning |
|-------|---------|
| `manual-budget` | `thinking:{type:"enabled", budget_tokens}` — claude-3*, claude-opus-4-5 |
| `adaptive` | `thinking:{type:"adaptive"}` + `output_config:{effort}` — opus-4-6+, sonnet-4-6+, etc. |
| `omit-fields` | Unknown Anthropic model — omit thinking fields rather than guess request shape |
| `none` | Non-Anthropic-routed model — thinking fields not applicable |
| `not-applicable` | Provider does not use Anthropic thinking API |

### `databricks_v2_wire_route` values

Scoped to DBv2 only. All non-DBv2 providers emit `not-applicable`.
Transport for pure OpenAI, legacy Databricks, and OpenRouter is selected by `OpenAiApi` /
`openai_request()` at runtime.

| Value | Meaning |
|-------|---------|
| `openai-responses` | `/ai-gateway/openai/v1/responses` |
| `anthropic-messages` | `/ai-gateway/anthropic/v1/messages` |
| `mlflow-chat` | `/ai-gateway/mlflow/v1/chat/completions` |
| `route-unknown` | DBv2 blank model — route not yet determinable |
| `not-applicable` | Not a DBv2 provider |

## Family rule match kinds

| Kind | Semantics |
|------|-----------|
| `exact` | Case-insensitive exact string equality on normalized alias |
| `prefix` | Normalized alias starts with match_value |
| `gpt5-token` | Boundary-aware token: present at end-of-string or followed by `-` (not digit/letter) |
| `gpt5-base` | Like gpt5-token but also rejects `-<1-3 digit>` suffixes (version-number rejection) |
| `segment` | Normalized alias contains match_value as a full alphanumeric segment (split on non-alnum) |
| `segment-prefix` | Any segment of the normalized alias starts with match_value |

## Boundaries the manifest does NOT own

- **Transport/endpoint selection for pure OpenAI, legacy Databricks, OpenRouter**: `OpenAiApi` and
  `openai_request()` remain authoritative. The `databricks_v2_wire_route` axis is DBv2-only.
- **Final display labels**: `resolveModelLabel(discovered_name, registry_label, raw_id)` three-tier
  precedence is authoritative. The manifest's `registry_label` feeds only the static registry tier.
- **`llm.rs` replacement scope**: only `databricks_v2_route_for_model`. Other dispatch paths remain.

## Reconciliation policy (plan v4 §Behavior policy)

Not purely behavior-preserving. `models.dev` `reasoning_options` become exact overrides. Each
divergence from family rule results is reconciled against provider docs and either:
- (a) **adopted** as an intentional correction with its own test + exact record, or
- (b) **rejected** with a curation note in the exact record.

See reconciliation table: `scripts/MODELS_DEV_RECONCILIATION.md`.

## Adding a new model family

1. Add a `family_rules` entry with a new unique `id`, appropriate `match_kind`, `providers`,
   `match_priority`, and all capability axes.
2. Run `node scripts/generate-model-capabilities.mjs` to regenerate artifacts.
3. CI `model-capability-regen-diff` job verifies byte-clean regeneration.
4. The normative corpus (`scripts/normative-corpus.json`) may need new vectors.

## Adding an exact model override

1. Add an `exact_records` entry with `provider` + `raw_model_id` (the full raw ID, no prefix
   stripping). Include a `_reconciliation` note and doc citation.
2. Run `node scripts/generate-model-capabilities.mjs` — completeness validator will fail if any
   axis cannot be resolved.
3. Regenerate and commit.
