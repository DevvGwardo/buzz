# Remote Agents and Their Management: A Formal Specification

`draft`

## Abstract

This document specifies the protocol by which Buzz Desktop delegates the
execution of a managed agent to a **remote substrate** — any compute
environment other than the local machine — through a **backend provider
binary**, and specifies the lifecycle contract every provider and every
remotely-run agent must satisfy. It covers three layers:

1. **The provider protocol** — a zero-registration plugin contract between the
   desktop and any executable named `buzz-backend-<id>`: discovery, the `info`
   and `deploy` operations, payload schema, and the security obligations on
   both sides of that boundary.
2. **The remote lifecycle model** — how a remote agent is started, observed,
   stopped, and reaped, given the deliberate design constraint that **the
   desktop holds no management channel to the remote process**. Relay
   presence is the sole status signal; shutdown is a relay message; liveness
   bounds are enforced by the agent harness itself, not by the desktop.
3. **The Kubernetes binding** — the first conforming provider,
   `buzz-backend-kubernetes`, which realizes the contract as a bare Pod
   running the `sprig` image.

We state five invariants — **identity fail-closed**, **no secrets in
configuration**, **presence-is-status**, **at-most-one-live-instance**, and
**bounded idle lifetime** — and argue each from the protocol rules. As with the git
specification (`git-on-object-storage.md`), naming the trust boundary is part
of the claim: a provider binary is arbitrary code that is handed an agent's
private key, and this document states exactly which properties hold *despite*
that, which hold only if the provider is honest, and which are explicitly the
user's acceptance.

## Scope and Non-Goals

This specification defines **management-plane behavior**: how agents get to a
substrate, how their state is observed, and how their lifetime is bounded. It
deliberately does **not** specify:

- **Agent conversational behavior.** What the agent does with events is
  governed by the ACP harness (`buzz-acp`) and the NIPs it implements
  (NIP-OA, NIP-AE, NIP-AA, …), unchanged by where the harness runs.
- **Malicious-provider containment.** A provider binary receives the agent's
  `nsec` by design — that is its job. The protocol *bounds the desktop's
  exposure* (discovery-only resolution, output caps, secret redaction,
  anti-secret config validation, an explicit UI trust warning) but cannot make
  a hostile provider safe. Choosing to run a provider is a trust decision the
  UI surfaces to the user; this document does not claim otherwise.
- **Substrate security.** Kubernetes RBAC, namespace isolation, and secret
  encryption at rest are cluster-operator concerns. The Kubernetes binding
  states its residual exposure (§K8s Secrets) rather than claiming isolation
  it does not provide.
- **Liveness of the substrate.** That a pod schedules, that an image pulls,
  that a cluster is reachable — empirical, not formal. The protocol specifies
  only how such failures are *reported* (structured error, redacted,
  fail-closed).

## System Model

Five principals:

- **Desktop** `D` — the Buzz Desktop app. Holds the agent's identity (nsec in
  the OS keyring), its configuration record, and the only UI. Trusted.
- **Provider** `P` — an executable `buzz-backend-<id>` on `D`'s machine.
  Invoked one process per operation: JSON request on stdin, JSON response on
  stdout, exit code meaningful. **Untrusted by `D`** for everything except
  the job it is explicitly given (deploying the agent, which requires the
  key). All of `P`'s output is treated as hostile (§Provider Output).
- **Substrate** `S` — the remote compute environment `P` deploys into (a
  Kubernetes cluster for the binding in this document). Opaque to `D`;
  `D` never talks to `S`.
- **Agent** `A` — a `buzz-acp` harness process (plus the ACP agent under it)
  running on `S`, holding the nsec it was given, connected to the relay.
- **Relay** `R` — the Buzz relay. The *only* channel that connects `D` to a
  running `A`. Everything `D` knows about a live remote agent, it learns
  from `R`.

The defining constraint, stated as a design axiom:

- **(M1) No management channel.** After a successful `deploy`, `D` holds no
  **persistent management session** to `A` on `S`, and the desktop↔provider
  protocol contains **no substrate API**: no status query, no exec, no log
  fetch, no kill. All post-deploy observation and control flows through `R`:
  status is relay presence (kind:20001), stop is a relay message
  (`!shutdown`), and reconfiguration is a future re-deploy. The reduction M1
  buys is **protocol surface, not credential absence**: ambient substrate
  credentials may well exist on `D`'s machine (the Kubernetes binding uses
  the user's kubeconfig by design), and `D` can always re-invoke `P`. What
  M1 guarantees is that nothing in *this protocol* — its persisted records,
  its wire operations, its stored `backend_agent_id` — constitutes or
  requires a channel to the substrate. The price is the staleness bounds in
  §Presence.

An agent's identity is a Nostr keypair. The **agent record** on `D` carries:
`name`, `relay_url`, the nsec (keyring-hydrated), the NIP-OA `auth` tag
attesting owner authorization, `agent_command`/`agent_args` (the ACP agent the
harness spawns — `goose`, `claude-agent-acp`, `codex-acp`, `buzz-agent`, or
any user-supplied command: this is the **configurable harness** requirement),
effective `system_prompt`/`model`/`provider`, timeout and parallelism knobs,
the `respond_to` gate, merged `env_vars`, and a `backend` discriminator:
`Local` or `Provider { id, config }`.

## Invariants

The protocol maintains five invariants. Each is stated with the mechanism
that enforces it and the boundary beyond which it does not hold.

A design obligation governs the whole list: **the complexity budget is
spent in this document, not in the code**. Every guarantee here was chosen
because its enforcing mechanism is one small, boring thing — a refusal at
payload construction (I1), a key-shape validator (I2), an ephemeral event
the agent already publishes (I3), a deterministic name plus one annotation
compare (I4), a timer that fires an existing shutdown channel (I5). The
same rule holds below: the deploy state machine is one loop over six
ordered rows; the Secret scheme is "unique name, write first, reference
exactly"; GC is one label-select with two filters (annotation, same-clock
age). Where
a richer property would have demanded machinery — Leases, controllers,
ownerReferences, a management channel — the spec either found a
name-and-timestamp argument that makes the machinery unnecessary or
dropped the property and said so (§Non-Goals, M1). A conforming
implementation that is not small is evidence of a spec bug; report it as
one.

- **(I1) Identity fail-closed.** No deploy request is ever emitted with an
  empty or missing private key. Enforced at payload construction: if keyring
  hydration left the nsec empty, `build_deploy_payload` refuses (mirroring
  local spawn's `spawn_key_refusal`). Boundary: a provider that *discards*
  the key and launches an identityless pod is a broken provider; I1 governs
  what `D` sends, not what `P` does with it.

- **(I2) No secrets in configuration.** `provider_config` — the persisted,
  schema-rendered, UI-visible settings object — MUST NOT carry secrets.
  Enforced by validation: flat object, scalar values only, ≤20 fields, ≤64KB,
  and any key whose word-split contains `secret|password|token|key|credential`
  is rejected. Secrets flow exclusively inside the `deploy` payload
  (`private_key_nsec`, `auth_tag`, `env_vars`), which is never persisted by
  `D` and never rendered. Corollary for providers: cluster credentials MUST
  come from ambient substrate config (e.g. kubeconfig resolution), never from
  `provider_config`.

- **(I3) Presence is the status.** `D` derives a remote agent's live state
  exclusively from relay presence events self-signed by the agent key:
  `online`/`away`/`offline` (kind:20001, ephemeral, WS-published). The
  deployment axis (`deployed`/`not_deployed`, from the stored
  `backend_agent_id`) is bookkeeping, not liveness. Staleness bound: presence
  can be wrong for the window between an abnormal agent death (SIGKILL, node
  loss) and the relay's presence expiry — this is the accepted cost of M1.
  The Kubernetes binding minimizes the *avoidable* part of that window by
  sizing the termination grace period to the harness's full graceful-shutdown
  path (§K8s Grace).

- **(I4) At most one live instance per agent key per deployment scope.**
  Within one provider's deployment scope (for Kubernetes: one namespace),
  there is never more than one Running instance of a given agent pubkey.
  Enforced by the deploy reconciliation loop (§Deploy State Machine):
  deploy is keyed on the derived pubkey, a live instance maps to strict
  no-op, and — because two deploys can race — create/delete conflicts MUST
  converge (re-read and return the winner) rather than fail; deterministic
  instance naming makes the substrate itself reject a second live instance.
  Boundary: the protocol cannot prevent the same nsec being
  deployed to two different scopes (two namespaces, two clusters, or remote
  + local simultaneously) — the relay tolerates multiple connections per key,
  and preventing this would require the global registry M1 forbids. Deploying
  one key twice is user error with confusing-but-safe results (both instances
  answer), not a safety violation.

- **(I5) Bounded idle lifetime.** No remote agent instance outlives its
  usefulness *silently*: an instance whose harness is live terminates on
  owner `!shutdown` or when inactivity exceeds a configured bound, and the
  substrate never resurrects a terminated instance. This is inactivity
  *reaping*, not an absolute TTL — a continuously active agent is
  intentionally unbounded (that is the product), and the requirement being
  satisfied is "stops after shutdown or 2h inactive", not "every instance
  terminates". Enforced *inside the harness* (the only place that can see
  activity, per M1) by the inactivity self-stop (§Auto-Stop), and made
  effective on the substrate by the binding's requirement that a terminated
  harness terminates its container (harness as the container's
  signal-receiving PID 1, `restartPolicy: Never`). Boundaries: (a) the
  guarantee is conditional on a live harness event loop — a wedged process
  that cannot run its reaper timer cannot reap itself, and M1 means
  nothing else will (the mitigation is the substrate operator's, e.g. a
  namespace-level TTL policy, out of scope per §Non-Goals); (b)
  `restartPolicy: Never` prevents resurrection, it does not prove process
  exit; (c) I5 bounds *agent* lifetime, not substrate residue — a Completed
  pod object persists for forensics until the next deploy's GC (§K8s GC).

## Provider Protocol

### Discovery

`D` scans, in order: the directory containing the desktop executable, every
entry of `PATH`, and `~/.local/bin`, for executables named
`buzz-backend-<id>`. The suffix after the prefix is the provider id and MUST
match `[a-z0-9][a-z0-9_-]*`. On Windows, an `.exe`/`.bat`/`.cmd` extension
MUST be stripped before the id is derived (see §Known Defects — as of
`c1bca1b56` it is not, so Windows providers probe but cannot deploy). First
hit per filename wins. Discovery executes nothing.

**Shadowing and invalid candidates are diagnosable, not silent.** First-hit
wins is the right selection rule (it is kubectl's), but kubectl also warns
when a later-PATH plugin is shadowed, and Docker's CLI reports invalid
plugin candidates with reasons. Discovery MUST retain, and the UI and
deploy-time errors MUST be able to surface: the selected binary's full
path, any shadowed candidates for the same id (later-PATH duplicates), and
candidates rejected for malformed names. A deploy error that names which
binary ran answers the first question a user with two copies of
`buzz-backend-kubernetes` will ask. (At `c1bca1b56` discovery records only
the winning path — a desktop change alongside Known Defect 3's.)

**Resolution rule.** Every subsequent operation resolves the provider id
against the *current* discovery set. A stored binary path on an agent record
is a cache, revalidated against both the current candidates and the recorded
id before every use. A record edit can therefore never redirect an operation
to a binary discovery would not have found.

**Pre-secret negotiation gate (normative).** Declaring `protocol_version`
is worthless if nothing checks it before the nsec crosses the trust
boundary — and at `c1bca1b56` nothing does: `provider_deploy` invokes
`deploy` directly, so a stale UI-time probe (or a binary replaced on PATH
since that probe) can receive `private_key_nsec` unchecked (Known Defect
5). The deploy path MUST: resolve the provider id **once**; invoke `info`
on that resolved executable; validate a compatible `protocol_version`; then
invoke `deploy` on the **same executable identity** — at minimum the same
canonical path with unchanged stable file identity/metadata (dev/inode or
equivalent, size, mtime) between the two invocations, failing or
re-prompting on any change. A UI-time probe result MUST NOT satisfy this
gate. Remembered digest-based approval (Terraform-lock style) is a stronger
follow-up, not a v1 requirement.

### Invocation

One process per operation. `D` spawns `P` with cwd = the agent workdir,
writes exactly one JSON object to stdin, closes stdin. `P` writes exactly one
JSON object to stdout and exits. Requirements on `D` (all implemented):

- Bounded reads: stdout capped (1MB), stderr capped (64KB), no `read_to_end`
  on pipes a daemonizing child could hold open; deadline polling with
  `try_wait`.
- **Non-zero exit is failure even if stdout parsed.** Partial output from a
  crashed operation is never trusted.
- `{"ok": false, "error": …}` is the in-band failure form.
- **Environment**: `P` inherits `D`'s environment. On macOS a GUI launch
  means launchd's minimal PATH; providers whose substrate credentials invoke
  helper binaries (kubeconfig `exec` plugins) MUST self-augment their PATH
  (§K8s Auth) rather than assume a login shell.

### Provider Output Is Untrusted

Everything `P` emits — stderr, error strings, the response object — is
scrubbed before storage or display: every value from the request's
`env_vars` (longest-first, length ≥4) and every `nsec1…`/`sprt_tok_…` token
is redacted. Rationale: `P` legitimately holds secrets during deploy; `P`
echoing them (in a stack trace, a kubectl error, a debug line) must not
propagate them into `D`'s persisted `last_error` or logs.

### `info`

```
request:  {"op": "info", "request_id": "<uuid>"}
response: {"ok": true, "name": str, "version": str,
           "protocol_version": int, "description": str,
           "config_schema": <JSON Schema>}
timeout:  10s
```

`version` is the provider's *software* version — useful in error reports,
useless for compatibility. `protocol_version` (this document: `1`) is the
wire-contract version, following the pattern Docker's CLI plugins
(`SchemaVersion`) and HashiCorp go-plugin (negotiated protocol version) both
converged on: the desktop rejects a provider whose `protocol_version` it
does not speak, with an error naming both versions and the binary path,
instead of failing later inside a half-understood `deploy`. A missing
`protocol_version` is treated as `1` for exactly one major cycle, then
becomes an error.

`config_schema` drives the UI form: `properties[*].default` prefill,
string/number/boolean coercion, `required` gating. A provider MAY compute
defaults freshly per call (the Kubernetes binding generates a random
namespace default this way — §K8s Namespace). The schema's fields are
subject to I2 validation when the user's values come back in `deploy`.

### `deploy`

```
request:  {"op": "deploy", "request_id": "<uuid>",
           "agent": <payload>, "provider_config": {…}}
response: {"ok": true, "agent_id": str}
timeout:  600s
```

The agent payload (field list per
`commands/agents_deploy.rs: deploy_payload_json` at `c1bca1b56`; the
`launch` block is a normative addition not yet emitted — Known Defect 3):

| field | meaning |
|---|---|
| `name` | display name |
| `relay_url` | concrete WS URL (workspace fallback materialized — the remote side has no workspace notion) |
| `private_key_nsec` | **the identity** (I1: never empty) |
| `auth_tag` | NIP-OA owner attestation |
| `agent_command`, `agent_args` | the ACP agent under the harness (configurable-harness support). At `c1bca1b56` these are raw record bytes — see Known Defect 3: the normative source is the resolved descriptor in `launch` |
| `system_prompt`, `model`, `provider` | effective values, live-persona-first resolution |
| `turn_timeout_seconds`, `idle_timeout_seconds`, `max_turn_duration_seconds` | harness timeout knobs |
| `parallelism` | concurrent-turn bound |
| `respond_to`, `respond_to_allowlist` | inbound author gate |
| `env_vars` | merged user env: global < persona < agent |
| `launch` | **normative addition** (§Launch data): the desktop-resolved launch contract — `command` (name, not path), normalized `args`, layered `env`, overridable `policy_env`, and `owner_pubkey` |

**Reserved-key rule (normative for providers).** `D` strips
`BUZZ_PRIVATE_KEY`, `NOSTR_PRIVATE_KEY`, `BUZZ_AUTH_TAG`, `BUZZ_RELAY_URL`,
and the other reserved keys from `env_vars` before merge. A provider MUST
construct the agent environment's identity variables from the **top-level**
payload fields (`private_key_nsec` → `BUZZ_PRIVATE_KEY`/`NOSTR_PRIVATE_KEY`,
`auth_tag` → `BUZZ_AUTH_TAG`, `relay_url` → `BUZZ_RELAY_URL`); reading
`env_vars` for them yields an identityless agent. A related hardening `D`
performs is part of the contract's rationale: env keys are validated as
POSIX-shaped names before merge, because a key like `BUZZ_AUTH_TAG=x`
smuggled through `Command::env` would bypass the reserved-key strip
entirely. A provider materializing `env_vars` into a substrate object
(e.g. a Kubernetes Secret) MUST likewise never let a user-supplied key
collide with or reconstruct a reserved key.

`agent_id` is `P`'s stable handle for the deployment (the Kubernetes binding
returns the pod name). `D` stores it as `backend_agent_id`; its presence is
the `deployed` axis of I3.

**There is no `undeploy` op in v1.** Deletion of a remote agent from `D`
orphans the substrate objects; the UI therefore requires an explicit
`force_remote_delete` confirmation, and the binding's GC + I5 bound the
orphan's cost (the agent self-stops; the pod residue is reaped on the next
deploy of the same key, or manually).

### Launch data (`launch`) {#launch-data}

Reproducing the local spawn's launch semantics requires state only the
desktop can resolve: the runtime-metadata table (`model_env_var`,
`provider_env_var`, `provider_locked`, `default_env` —
`discovery.rs:74-193`), the six-layer env resolution, harness-definition
command/args fallback, team instructions, session title, the respond-to
gate's legacy owner fallback, and the mesh rewrite. A provider MUST NOT
reimplement that derivation — it would be a second copy of desktop runtime
discovery, drifting from the first. Instead the payload carries a typed
`launch` block that `D` resolves with **the same code paths as local
spawn**, and the provider applies it mechanically.

```
"launch": {
  "command":      str,          // command NAME (e.g. "goose"), never a host path
  "args":         [str],        // normalized args, definition fallback applied
  "env":          {str: str},   // layered env: baked → runtime metadata →
                                // definition → global → persona → agent
                                // (resolve_effective_harness_descriptor)
  "policy_env":   {str: str},   // overridable behavior defaults (tier 1, below):
                                // runtime default_env (e.g. GOOSE_MODE=auto),
                                // BUZZ_ACP_RELAY_OBSERVER, BUZZ_ACP_LAZY_POOL=true,
                                // BUZZ_ACP_SESSION_TITLE (resolved),
                                // BUZZ_ACP_TEAM_INSTRUCTIONS, BUZZ_ACP_MODEL,
                                // MCP_HOOK_SERVERS=* (mcp_hooks runtimes only)
  "owner_pubkey": str | null    // resolved workspace owner (hex) — legacy
                                // BUZZ_ACP_AGENT_OWNER fallback, non-secret
}
```

`launch.command`/`launch.args` come from
`resolve_effective_harness_descriptor` (`readiness.rs:125`) — the same
resolver local spawn uses — which fixes two silent divergences the raw
record fields carry: a persona-derived `agent_command` is a blank record
byte, and definition-provided `agent_args` are lost when the instance's own
args are empty. `launch.env` is that descriptor's layered env, which is
where per-runtime model/provider injection lives (`GOOSE_MODEL`/
`GOOSE_PROVIDER` for goose; nothing for `provider_locked` runtimes like
Claude; `BUZZ_AGENT_MODEL`/`BUZZ_AGENT_PROVIDER` for buzz-agent). A fixed
`provider → BUZZ_AGENT_PROVIDER` mapping is wrong for three of the four
built-in runtimes and is why this block exists.

**What `policy_env` carries — and deliberately does not.** Its irreducible
wire fields are exactly three scalars plus the metadata-derived defaults —
plus the four record-derived behavior knobs that would otherwise be
mis-tiered (below):

- `BUZZ_ACP_TEAM_INSTRUCTIONS` — the only truly non-reconstructible policy
  value: `effective_team_instructions` (`spawn_hash.rs:41-52`) needs the
  desktop's `TeamRecord` store, which no pod can reach.
- `BUZZ_ACP_SESSION_TITLE` — sent **resolved** (`resolve_session_title`,
  `runtime/metadata.rs:45`), not as its `display_name`/`name` inputs. The
  resolution strips control characters, and that property transfers: an
  interior NUL fails a local spawn at the env boundary, and would make the
  Kubernetes apiserver reject the whole pod spec — a rename must degrade,
  not turn into a deploy failure.
- `owner_pubkey` (block-level, not env) — the respond-to gate is otherwise
  fully reconstructible from payload fields (`build_respond_to_env`,
  `runtime.rs:380-421`); this is its one irreducible input.
- Runtime `default_env` (e.g. `GOOSE_MODE=auto`) — computed **from the
  runtime metadata table only, unconditionally**. The local spawn applies
  each default only `if std::env::var(key).is_err()` (`runtime.rs:733-737`)
  — a test of the *desktop's own* ambient environment. That makes "the
  resolved local env" not a pure function of the record; serializing it
  verbatim would bake a host accident into the pod. Launch data MUST be
  computed from record + config alone.
- `BUZZ_ACP_LAZY_POOL=true` — a **deliberate pick, not a transcription**:
  the two local paths disagree (manual Start is eager, `runtime.rs:1006`;
  launch restore is lazy, `restore.rs:333`, precisely to avoid "N idle
  brains on every launch"). Remote pods take the lazy arm: an idle LLM pool
  in a cluster is billable waste with no user watching it warm up.
- `MCP_HOOK_SERVERS=*` when the resolved runtime has `mcp_hooks`
  (`runtime.rs:594-598`; buzz-agent only at `c1bca1b56`) — gates the
  `_Stop`/`_PostCompact` hook tools.
- `BUZZ_ACP_SYSTEM_PROMPT`, `BUZZ_ACP_IDLE_TIMEOUT`,
  `BUZZ_ACP_MAX_TURN_DURATION`, `BUZZ_ACP_AGENTS` — resolved by the desktop
  from the record's `system_prompt` / `idle_timeout_seconds` /
  `max_turn_duration_seconds` / `parallelism` (each omitted when null,
  matching the local spawn's conditional emission). These are **tier-1 by
  local fact, not by choice**: the local spawn writes them before the user
  env layer (`runtime.rs:716-729,763` vs `:860`) and none is in
  `RESERVED_ENV_KEYS`, so a power user's env override beats them today. A
  provider that independently mapped the top-level payload copies after
  `launch.env` would invert that — the structured field silently defeating
  an override that works locally — which is why the provider MUST NOT remap
  them (§Entrypoint mapping table).

`BUZZ_ACP_DEDUP` and `BUZZ_ACP_MULTIPLE_EVENT_HANDLING` are **deliberately
unset**: the local spawn writes `queue`/`steer` (`runtime.rs:730-731`), and
those are exactly the harness's clap defaults (`config.rs:344,356`) — a pod
that omits both is behaviorally identical, and adding rows for them would
imply a divergence that does not exist. `BUZZ_MANAGED_AGENT` is likewise
deliberately absent remotely: it brands local harness processes so the
desktop's orphan sweep and instance reaper can prove ownership by scanning
process env (`orphan_sweep.rs`, `instance_reaper.rs`) — there is no local
process to sweep.

**Environment precedence (normative) — three tiers, later wins:**

1. **Overridable behavior defaults** — `launch.policy_env`. These keys are
   deliberately non-reserved (`env_vars.rs:54-57` says so outright: power
   users may bypass the dedicated UI fields), and locally the user env is
   written after them (`runtime.rs:860` and its comment). A policy-wins
   order here would make remote agents ignore overrides local agents honor.
2. **User/layered env** — `launch.env`. User `env_vars` need no separate
   slot: the descriptor's layering already merged them (global < persona <
   agent), so a provider applies `launch.env` and MUST NOT re-merge the
   legacy `env_vars` field on top.
3. **Authoritative** — unoverridable at every layer, written last and
   backed by the reserved-key strip: the identity variables from top-level
   payload fields (§Reserved-key rule), the respond-to gate values,
   `BUZZ_ACP_AGENT_OWNER`, the inactivity bound, `BUZZ_ACP_MCP_COMMAND`,
   and `BUZZ_MANAGED_AGENT_START_NONCE`. For the nonce, the provider MUST
   set it to the attempt's **generation token** (§K8s Secrets): the harness
   stamps it into every observer lifecycle frame (`buzz-acp/lib.rs:1501`),
   so the Secret generation and the lifecycle correlator become one
   identity instead of an empty string.

**Host-resolved values MUST NOT be forwarded and MUST be re-derived
in-image.** The local spawn sets several variables to absolute paths on the
desktop's filesystem; forwarding them into a container is a guaranteed
failure. The provider/image re-derives:

- the harness and agent binaries: `launch.command` is a *name*, resolved
  against the image's own `PATH` (`BUZZ_ACP_AGENT_COMMAND`), and
  `BUZZ_ACP_MCP_COMMAND=buzz-dev-mcp` likewise;
- `CLAUDE_CODE_EXECUTABLE` — a `resolve_command()` host path
  (`configure_runtime_cli`, `runtime.rs:424-446`), same class as the
  command paths: image-local resolution or unset;
- `PATH` itself (the desktop's augmented PATH is meaningless in the image);
- git credential/signing helper locations — the relay-URL *scoping* of the
  credential config is normative (never a global helper), the helper *path*
  is image-local (§Image);
- `BUZZ_ACP_SETUP_PAYLOAD` is desktop-computed readiness state and MUST NOT
  appear in a remote pod.

**Owner resolution (normative):** the provider MUST have either a non-null
`auth_tag` (→ `BUZZ_AUTH_TAG`) or a non-null `launch.owner_pubkey`
(→ `BUZZ_ACP_AGENT_OWNER`) before any mutation; if both are null it MUST
refuse the deploy. Without an owner the harness cannot match `!shutdown`
(`buzz-acp/src/lib.rs: resolve_agent_owner`, main-loop owner check) and the
agent answers its own stop command conversationally — §Stop would be
describing a mechanism that does not work. `BUZZ_ACP_AGENT_OWNER` is a
reserved key, so this value can only arrive as authoritative launch data,
never through user env.

**Buzz shared compute (relay-mesh) is non-deployable, and this is forced,
not chosen.** The mesh rewrite resolves to an OpenAI-compatible transport at
`http://127.0.0.1:9337/v1` (`relay_mesh.rs: RELAY_MESH_API_BASE_URL`) — a
loopback proxy on the desktop. Serializing that policy into a pod points the
agent at its own localhost, where nothing listens. `D` already rejects
mesh-configured creates on non-local backends
(`agents.rs: normalize_relay_mesh`); the deploy path MUST equally fail
closed — before any mutation — when the effective provider resolves to
`relay-mesh`, rather than passing `relay-mesh` through as if it were a
runtime provider. Remote mesh transport is a possible v2 (an in-image mesh
client), not a v1 silent breakage.

**The governing invariant:** a remote agent's environment differs from the
same record's local spawn **only where the substrate forces it** (paths,
PATH, readiness). Anyone adding a local behavior knob adds it to the shared
resolver, and both spawn paths inherit it; there are not two derivations to
keep in sync.

### Deploy State Machine

`start` on any non-Local agent unconditionally issues `deploy` — the desktop
does not track substrate state (M1). Deploy is therefore **not** "create": it
is *converge to at-most-one-live-instance* (I4), implemented as a
**reconciliation loop** keyed on the agent's identity within the provider's
scope.

**Step 0 — derive and verify identity.** The payload carries the nsec, not
the pubkey. Before any substrate read or mutation, the provider MUST parse
`private_key_nsec` and derive the public key from it; a malformed or
undecodable key is an immediate in-band error. Every selector, name, and
comparison below uses the *derived* pubkey — never a caller-supplied one.

**Step 1 — select and authenticate candidates.** Candidate objects are
selected by the (truncated) identity label, then each candidate's
**full-pubkey annotation MUST be compared against the derived pubkey**
before it is treated as belonging to this agent. Truncated selectors are
collision-*resistant*, not collision-*free*: the annotation check is what
makes them safe. An object whose annotation does not match MUST NOT be
no-op'd against, deleted, GC'd, or have its Secret touched; the provider
MUST either ignore it or fail with an explicit collision error. Only
annotation-verified objects proceed.

**Step 2 — reconcile.** Ordered rules, evaluated against the verified
observation; on any conflict, *re-enter from step 1* rather than fail:

| observed | action | rationale |
|---|---|---|
| instance marked for deletion (`deletionTimestamp` set, any phase) | wait for actual disappearance, then re-enter | the user pressed Start and the old instance is unrecoverable; returning the dying instance's id records a success that evaporates. **Note: in Kubernetes there is no `Terminating` phase — a pod being gracefully deleted stays in phase `Running` for its whole grace period.** The deletion mark MUST be checked *before* phase, or this row is mistaken for the no-op row |
| no instance | create, then verify startup (below) | first deploy / after GC |
| terminated (Succeeded/Failed) | delete residue, wait for disappearance, re-enter (→ create) | the **normal restart path**: how a user revives a reaped or shut-down agent |
| live and **started** (harness container running) | **strict no-op; return existing `agent_id`** | Start must never silently kill a live agent mid-turn; "already running" is the honest answer, consistent with I3 |
| exists but **never started**, provably non-recoverable — referenced Secret confirmed absent (by a consistent read, below), or invalid image reference | delete (preconditioned, below), wait for disappearance, re-enter (→ create) | a pod whose harness never ran is not a live agent: nothing can be killed mid-turn (I3), it never held the identity (I4), auto-stop cannot bound it (I5's reaper lives in the harness), and no-op'ing it would return a permanently inert instance as success on every future Start. "Provably" means the provider verified the referenced object's absence or the spec-level defect itself — never a reason string alone |
| exists, **never started**, recoverable — self-healable startup states: `Unschedulable` (scale-from-zero autoscaling), image pull / `ImagePullBackOff`, transient `CreateContainerConfigError` | observe until started or the operation deadline expires, then return the latest redacted condition — **never delete, on this call or any later one** | these states routinely self-heal — an autoscaler provisions the node, the pull retries, the kubelet re-resolves the Secret (it retries a never-created container regardless of `restartPolicy`). And recoverable-timeout MUST stay observational *across calls*: any finite pod-age threshold can collide with the cluster's own pod-age thresholds (Cluster Autoscaler's `--new-pod-scale-up-delay` / per-pod `pod-scale-up-delay` annotation — the FAQ's example is `"600s"`), and delete-recreate resets exactly the age the autoscaler keys on, converting a slow cold start into a livelock in which every individual decision is correct. A later deploy re-reads: started → strict no-op; still recoverable → observe under the new call's deadline without resetting pod age; provably non-recoverable → the row above. A permanently unschedulable pod persisting until operator action is the honest cost — a generic provider cannot know when an autoscaler will act, and M1 already makes substrate residue the operator's boundary |

**Startup is part of create — phase is not readiness.** `Pending` (and even
`Running` at the pod level) does not mean the harness started: a pod can sit
in `ImagePullBackOff`, `CreateContainerConfigError` (e.g. a missing
`envFrom` Secret), or unschedulable `Pending` forever, and I5's inactivity
reaper cannot bound a harness that never began. Therefore `deploy` MUST NOT
report success at pod acceptance: it succeeds only when the harness
container has actually started (container `state.running`), bounded by the
operation deadline. On failure or deadline expiry it MUST return an in-band
error carrying the actionable condition (the container waiting `reason` /
pod condition), not a generic timeout. "Live" in the no-op row above means
**started**, for the same reason — this is the lesson ephemeral-runner
controllers learned upstream (inspect container state, not pod phase).
Classification MUST combine container state, pod conditions, and
referenced-object existence — **reason strings alone are not a
stable fatality taxonomy**, and pod age is never one (age triggers nothing
destructive; see the recoverable row and the controlled-view rule below).
In particular, `Unschedulable` is not fatal: a
scale-from-zero pod reports it while the autoscaler provisions capacity,
and the kubelet retries a container that never got a container status
regardless of `restartPolicy: Never` (`ShouldContainerBeRestarted` returns
true for a nil status *before* the restart-policy check —
`kubelet/container/helpers.go`), which is exactly why a briefly-missing
Secret self-heals. `restartPolicy: Never` suppresses restarting a container
that ran and died; it says nothing about one that never started.
A consequence to state plainly: once success includes container start, the
600s operation deadline **is** the cold-start budget — image pull on a
fresh node, scale-from-zero scheduling, all of it. But the deadline bounds
**how long one Start waits synchronously**, nothing more. Deadline expiry
on a still-progressing startup is reported as "startup not confirmed within
the deadline", and MUST NOT trigger cleanup or forced recycle — on this
call *or any later one* (the recoverable row above): the next deploy's
reconciler observes whatever the startup became and takes the matching row,
preserving the pod's `creationTimestamp` for whatever cluster machinery
keys on it. Whether ten minutes fits the intended cluster class is a
product ruling,
not a correctness one.

**Destructive decisions come from views you control — reads and writes
both (normative).** This is one rule with three instances, stated once so
nobody optimizes an instance away. §K8s GC's same-clock rule is the time
instance. The other two live here:

- *Reads*: every read whose result can authorize a deletion — the
  Secret-absence confirmation above, and the candidate list the GC pass
  filters — MUST use most-recent semantics (`resourceVersion` **unset**,
  a quorum read). `resourceVersion: "0"` is served from the watch cache,
  which the Kubernetes API contract explicitly allows to be much older
  than anything the client has already observed; a stale
  Secret-absence read would delete a pod whose Secret exists and whose
  container was about to start — the GC race again, arriving through read
  consistency instead of a clock.
- *Writes*: a fresh read is necessary but not sufficient — the kubelet can
  start the container between observation and delete. Every DELETE
  authorized by a classification MUST carry `preconditions.uid` **and**
  `preconditions.resourceVersion` from that exact observation
  (`metav1.Preconditions` supports both), making the edge a
  compare-and-delete. A failed precondition (409) is neither an error nor
  permission to retry the delete: re-enter from step 1 and classify the
  object that exists now. The full-pubkey annotation check remains — the
  precondition pins *when*, the annotation pins *whose*.

**No-op means zero mutation.** The live-instance row MUST NOT replace or
patch the Secret, patch metadata, or delete anything belonging to the
observed live generation. Configuration and environment edits apply only to
the *next* fresh generation (see the documented consequence below).

**Conflicts converge, never fail.** Two provider processes can concurrently
observe "no instance" or "terminated" — the deterministic instance name
prevents two live instances, but one caller loses the race. The provider
MUST treat create-conflict (already exists) by re-reading and, if the winner
is an annotation-verified live instance, returning it as the no-op row
would — cleaning up only its own losing attempt's residue, never the
winner's (the Kubernetes binding makes this concrete via per-attempt Secret
names, §K8s Secrets); it MUST treat delete-not-found as success; and it
MUST loop until a
stable outcome or the operation deadline (600s) expires. Without this rule,
"two deploys return an `agent_id`" (the idempotency claim below) is false
under concurrency.

**Documented consequence.** Because live → no-op, configuration edits to a
running remote agent do not take effect until it next exits (unlike local
agents, which re-resolve on every spawn). This is an accepted v1 tradeoff;
a deliberate "recycle" affordance (stop-then-start) is the v2 path to
immediate application. [DECISION — default is no-op; owner may overrule
toward forcible recycle.]

Idempotency in the protocol sense: any number of concurrent or sequential
`deploy`s with the same payload converge to one live instance, and every
non-erroring call returns an `agent_id` naming it; no sequence of `deploy`s
can yield two live instances in one scope.

### Stop and Delete

- **Stop** is not a provider operation. `D` publishes `!shutdown` mentioning
  the agent on `R`; the harness verifies the sender is the owner and exits
  through its graceful path: agent-pool shutdown (unbounded in principle,
  short in practice), then a bounded tail of drain in-flight turns ≤30s,
  publish presence `offline` ≤2s, close relay connection ≤5s — **~37s is the
  bounded tail, not a proven upper bound on the whole path**. §K8s Grace
  sizes for this; anyone tuning a grace period downward from "37s" is
  reading the number without its qualifier. The desktop's local stop command
  rejects remote agents.
- **Delete** with a live `backend_agent_id` requires `force_remote_delete:
  true` from the UI's orphan-warning confirmation — a buggy IPC caller
  cannot silently orphan substrate objects.

### Auto-Stop (Inactivity Self-Termination)

I5's enforcement point. A new harness knob:

```
--exit-after-inactivity <secs>   /   BUZZ_ACP_EXIT_AFTER_INACTIVITY
```

- **Default 0 = disabled.** The flag ships in the harness every *local*
  agent also runs; a reaper bug must not be able to kill a laptop agent.
  Remote providers opt in (the Kubernetes binding's `inactivity_seconds`
  config field, schema default 7200 = 2h, feeds this env var directly).
- **"Inactivity" is defined as**: no events dispatched to the agent and no
  turns in flight. Raw relay traffic does not count — an agent lurking in a
  busy channel it never answers is exactly the waste this bounds.
- **Mechanism**: on expiry of the bound, the harness fires the same shutdown
  channel `!shutdown` uses — so inactivity exit gets in-flight drain,
  presence→offline, and graceful relay close identically to an owner stop.
  **The expiry check MUST NOT depend on pool readiness.** This is a design
  constraint learned by inspection, not a transcription: the harness's
  existing 30s maintenance tick is gated on `pool_ready` (`lib.rs:1743`),
  which under `lazy_pool` starts false (`:1320`) and flips true only on a
  wake (`:2570`) — and wakes require pending work (`pool_lifecycle.rs:42`).
  A reaper riding that tick composes with the mandated
  `BUZZ_ACP_LAZY_POOL=true` (§Launch data) into a deadlock in I5's single
  most important case: a never-mentioned lazy pod never runs the tick, so
  the idle agent the reaper exists to kill is exactly the one it can never
  evaluate. The reaper therefore runs on its own timer, independent of pool
  state (an idle-pool check needs no pool). Check granularity makes the
  effective bound `t ∈ [T, T+interval)`, immaterial at T=7200.
- **Reserved key**: `BUZZ_ACP_EXIT_AFTER_INACTIVITY` MUST join
  `RESERVED_ENV_KEYS` (`env_vars.rs`) when it lands — it is tier-3
  authoritative (§Launch data), and without reservation a user env var
  could disable the reaper and reopen unbounded lifetime through the front
  door.
- Distinctness note: this is a **fourth** timeout concept, deliberately named
  away from the existing three (`--idle-timeout` = per-turn ACP wire silence,
  900s; `turn_timeout`; `max_turn_duration` = 7200s — numerically equal to
  the default inactivity bound and semantically unrelated). Sharing a flag or
  env name with any of them is how the bug ships.

The harness exiting MUST terminate the container, and — equally load-bearing
— the harness MUST be the container's **signal-receiving process** (PID 1 or
the target of the substrate's termination signal). A wrapper that runs the
harness as a child without forwarding signals silently voids both I5's
substrate half *and* the graceful-shutdown budget: the termination signal
lands on the wrapper, the harness never learns to shut down, and the
force-kill leaves presence stale-online — exactly the staleness window the
grace period exists to close. See §K8s Entrypoint for the concrete rule.
With `restartPolicy: Never`, harness exit completes the pod — turning
agent-level I5 into substrate-level I5.

## The Kubernetes Binding (`buzz-backend-kubernetes`)

The first conforming provider: a Rust crate in `block/buzz`, distributed as a
standalone binary. Everything above is the contract; this section is its
realization.

### Cluster auth {#k8s-auth}

Standard kubeconfig resolution (`$KUBECONFIG` → `~/.kube/config`) via
`kube-rs`. `provider_config` carries **`context`** and **`namespace`** only
(I2: credentials never transit config). Because kubeconfigs at Block
near-universally use `exec` credential plugins (`aws eks get-token`,
`gke-gcloud-auth-plugin`) that resolve via PATH, and the provider inherits a
Finder-launched desktop's minimal PATH, the provider MUST prepend
`/opt/homebrew/bin`, `/usr/local/bin`, and `~/.local/bin` to its own PATH
before building the client, and on exec-plugin failure MUST name the missing
plugin binary in the error rather than surfacing a kube-rs stack.

### Namespace {#k8s-namespace}

One stable namespace per user-visible choice; the provider emits a freshly
generated `buzz-agents-<rand6>` as the `namespace` field's schema *default*
on every `info` call, so the UI prefills a visible, editable random name with
zero UI changes ("random default" satisfied at the schema layer). If the
namespace does not exist the provider attempts to create it; on RBAC denial
it MUST fail with the literal `kubectl create namespace <name>` command to
run — it MUST NOT fall back to `default`.

### Image

`ghcr.io/block/buzz-sprig`: Alpine base + `bash` (required by the dev-MCP
shell tool) + `git` + CA certificates + the static musl `sprig` multicall
binary with its personality links (`buzz-acp`, `buzz-agent`, `buzz-dev-mcp`,
`rg`, `tree`, `buzz`, `git-credential-nostr`, `git-sign-nostr`) + a baked
system gitconfig wiring the nostr signing and credential helpers. The baked
credential-helper config MUST be scoped to the relay's git URL — mirroring
the local spawn's `credential.<relay-url>/git.helper` scoping — never a
global `credential.helper`: a global nostr helper would answer for every
remote, including github.com. ~15–25MB;
not FROM-scratch (bash and git preclude it). Sprig-only: alternate-harness
dependencies (node for Claude Code / Codex) come via the `image` override
field, not a fatter default. Tagging follows the relay image's matrix —
`sha-<short>` on main, semver on `sprig-v*` tags (the sprig tarball's
`+git.<sha>` version string is not a legal Docker tag). **The default image
reference MUST be pinned by digest, not tag**: the provider bakes, at
compile time, the multi-arch manifest digest of the image built from its
own commit and defaults `image` to
`ghcr.io/block/buzz-sprig@sha256:<that-digest>` — a `sha-<git-sha>` *tag*
is traceable but still movable (registry tags are mutable pointers;
Kubernetes distinguishes movable tags from immutable digests for exactly
this reason), and the object holding it runs with an nsec. The provider
records the reference it used in a pod annotation, and rejects `:latest`.
User `image` overrides accept tag, digest, or full custom registry
reference — visibly the user's trust decision, with the resolved image ID
recorded in the same annotation for post-hoc attribution.
**An image override MUST contain the runtime ABI** — the `buzz-acp`
entrypoint and everything §Entrypoint and launch ABI requires — not merely
alternate-harness dependencies. A conforming custom image is "buzz-sprig
plus your tools", never "your tools instead".

### Entrypoint and launch ABI {#k8s-entrypoint}

Two conforming implementations must produce interchangeable pods, so the
launch contract is normative.

**Entrypoint.** The container runs the harness as its signal-receiving
process. Sprig is a multicall binary with no supervisor personality —
nothing reaps children or forwards signals — so the entrypoint MUST end in
`exec`:

```bash
#!/bin/bash
set -e
# nest scaffolding, if DECISION A lands, goes here
exec buzz-acp   # exec, not a call — buzz-acp must be PID 1
```

`bash -c "setup && buzz-acp"` (no `exec`) is non-conforming: bash becomes
PID 1, and a PID-1 bash with no trap never delivers SIGTERM to the harness
(PID 1 receives kernel-level default-handler signal immunity), so the pod
rides out the entire grace period and is SIGKILLed with presence still
online — voiding I5's substrate half and the very staleness window
`terminationGracePeriodSeconds: 60` was sized to close. The entrypoint
shape and the grace period are one requirement, not two.

**Payload → environment mapping.** The provider builds the pod environment
(via the per-agent Secret, §K8s Secrets) by applying the §Launch data
three-tier precedence — `launch.policy_env` (overridable defaults), then
`launch.env` (user/layered), then the authoritative tier from top-level
fields per the reserved-key rule. Only the
non-`launch` scalars and the substrate-forced re-derivations are mapped
individually:

| source | env var |
|---|---|
| `relay_url` | `BUZZ_RELAY_URL` |
| `private_key_nsec` | `BUZZ_PRIVATE_KEY` and `NOSTR_PRIVATE_KEY` (the git helpers read the latter) |
| `auth_tag` | `BUZZ_AUTH_TAG` (omitted when null; then `launch.owner_pubkey` → `BUZZ_ACP_AGENT_OWNER` is REQUIRED — §Launch data owner rule) |
| `launch.command` | `BUZZ_ACP_AGENT_COMMAND` — the *name*, resolved against the image's own PATH; never a forwarded host path |
| `launch.args` | `BUZZ_ACP_AGENT_ARGS`, comma-joined |
| `launch.env`, `launch.policy_env` | verbatim, at their precedence tiers |
| generation token (§K8s Secrets) | `BUZZ_MANAGED_AGENT_START_NONCE` — the lifecycle-frame correlator and the Secret generation are one identity (§Launch data tier 3) |
| `system_prompt`, `idle_timeout_seconds`, `max_turn_duration_seconds`, `parallelism` | **not mapped by the provider** — the desktop resolves these into `launch.policy_env` (`BUZZ_ACP_SYSTEM_PROMPT`, `BUZZ_ACP_IDLE_TIMEOUT`, `BUZZ_ACP_MAX_TURN_DURATION`, `BUZZ_ACP_AGENTS`), because they are tier-1 behavior knobs: locally they are written *before* the user env layer and none is reserved (`runtime.rs:716-729,763` vs `:860`; `env_vars.rs:54-57`), so user env beats them. A provider that mapped the top-level copies after `launch.env` would silently defeat an override that works locally. The top-level fields remain as display/bookkeeping inputs only |
| `turn_timeout_seconds` | not mapped — deprecated upstream and ignored; the local spawn also does not emit it |
| `respond_to` | `BUZZ_ACP_RESPOND_TO` |
| `respond_to_allowlist` | `BUZZ_ACP_RESPOND_TO_ALLOWLIST`, comma-joined |
| — | `BUZZ_ACP_MCP_COMMAND=buzz-dev-mcp` (image-local; the dev-MCP requirement) |
| `provider_config.inactivity_seconds` | `BUZZ_ACP_EXIT_AFTER_INACTIVITY` (schema default 7200; the I5 opt-in, §Auto-Stop — the config field and this env var are one knob, not two) |

The top-level `model`/`provider` payload fields are display/bookkeeping
inputs; the *environment* consequence of model and provider selection
(per-runtime vars, `provider_locked` suppression, `BUZZ_ACP_MODEL`) arrives
resolved inside `launch.env`/`launch.policy_env`. A provider MUST NOT map
`provider` to any env var itself — that mapping is per-runtime and lives in
the desktop's resolver (§Launch data).

**Encoding honesty note.** `BUZZ_ACP_AGENT_ARGS` is comma-delimited by the
harness's CLI parser, and the desktop's *local* spawn performs the same
comma-join — an argument containing a comma is unrepresentable in both
paths. This is a harness interface limitation the binding inherits and
matches, not one it introduces; a provider MUST NOT invent a private
escaping scheme the harness would not decode.

**Working directory.** `HOME` is set to a writable path backed by the
workspace `emptyDir` (e.g. `/home/agent`), and the harness runs with cwd =
`HOME` — mirroring the local spawn's agent-workdir convention. The baked
system gitconfig references the nostr helpers by absolute path so it works
regardless of `HOME`.

### Pod shape

- **Bare Pod, `restartPolicy: Never`.** Eviction → presence `offline` (I3)
  → user hits Start → the reconciler's terminated arm re-creates. No Job, no
  controller: a restart controller would resurrect what `!shutdown` and
  auto-stop terminate, violating I5.
- **Naming/labeling — the exact contract** (63-char label-value limit; a hex
  pubkey is 64 chars, one over):
  - pod name: `buzz-agent-<first-12-hex-of-pubkey>` — also the returned
    `agent_id`
  - label `buzz.block.xyz/agent-pubkey: <first-32-hex>` — the selector key
    for reconciliation and GC. 128 bits is collision-*resistant*, not
    collision-free, which is why the annotation check below is normative,
    not decorative
  - annotation `buzz.block.xyz/agent-pubkey-full: <full-64-hex>` —
    **load-bearing**: per §Deploy State Machine step 1, every label-selected
    object's annotation MUST equal the derived pubkey before the provider
    no-ops against it, deletes it, mutates its Secret, or returns its name
  - Secret name: `buzz-agent-<first-12-hex>-<gen>`, where `<gen>` is a random
    per-create-attempt **generation token** — unique, never reused, carrying
    the same label and annotation. The pod's `envFrom` references this exact
    Secret name. Deterministic pod name + unique Secret name is what makes
    payload and Secret atomic at the pod-spec boundary (§K8s Secrets)
- **Deletion semantics the reconciler must respect.** A Kubernetes `DELETE`
  returns success immediately while the object still exists; the name stays
  taken until the kubelet finishes the grace period. Two consequences:
  (a) a pod being gracefully deleted has `deletionTimestamp` set but remains
  in phase `Running` — the reconciler MUST check the deletion mark before
  phase (there is no `Terminating` phase to match on); (b) after deleting a
  live pod, a naive immediate create gets AlreadyExists for up to the full
  grace period — the reconciler MUST poll for actual disappearance (GET →
  404) before creating. The delete call MUST use the object's own grace
  period (kube-rs: `DeleteParams { grace_period_seconds: None, .. }`); the
  tempting shortcut of passing `0` to skip the poll is a **force-kill** that
  discards the 60s shutdown grace pinned below — the poll is mandatory
  precisely because the fast path is wrong. For *terminal*
  (Succeeded/Failed) pods — and for **unscheduled** pods (no assigned node:
  unschedulable or quota-blocked `Pending`, a state users hit while setting
  up a namespace) — the apiserver
  zeroes the grace period and deletes immediately, so the normal restart
  path needs no meaningful wait — do not add a fixed sleep, and do not use
  zero-grace cleanup as a reason to skip the poll in the live-pod arm.
- **`terminationGracePeriodSeconds: 60`.** The harness's graceful shutdown
  has a ~37s **bounded tail** (30s drain + 2s presence + 5s relay close)
  after an agent-pool shutdown that is not itself bounded; Kubernetes'
  default 30s grace would SIGKILL it mid-drain, leaving presence
  stale-online — the avoidable half of I3's staleness window. 60s is a
  chosen operational margin over the bounded tail, not a proven upper bound.
- **Hardening defaults (normative).** The workload is a prompted coding
  agent running repository and tool code while holding an nsec; the pod MUST
  NOT hand it ambient cluster credentials or kernel privilege on top:
  `automountServiceAccountToken: false` (Kubernetes mounts a ServiceAccount
  token unless told otherwise — an API-stealable credential the agent never
  needs), `runAsNonRoot: true` with a fixed nonzero UID/GID,
  `allowPrivilegeEscalation: false`, capabilities drop-all,
  `seccompProfile.type: RuntimeDefault`; never privileged, `hostPID`,
  `hostNetwork`, or `hostPath`. `readOnlyRootFilesystem` is *not* required
  in v1 — the sprig toolchain writes outside the workspace mount — but is a
  named candidate once the image's write surface is mapped. The
  `service_account` config field selects an identity for scheduling/RBAC
  purposes only; it MUST NOT silently re-enable token mounting — API-token
  access, if ever wanted, is a separate explicit opt-in, not a side effect
  of naming an SA.
- **Resources**: requests 1 cpu / 2Gi, limits 2 cpu / 4Gi, all four
  configurable (`cargo build` in an agent workspace makes 500m/1Gi requests
  unrealistic).
- **Workspace**: `emptyDir`. Checkouts and scratch die with the pod; agent
  memory is relay-persisted (NIP-AE) and unaffected. PVC support is a
  deferred knob. [DECISION A — whether the image entrypoint scaffolds the
  nest workspace (AGENTS.md etc.), which local agents get from the desktop's
  `ensure_nest` and remote pods currently would not.]

### Secrets {#k8s-secrets}

Per-agent `Secret` containing the identity variables (built from top-level
payload fields per the reserved-key rule) plus `env_vars`; consumed via
`envFrom`.

**Secret creation is per-attempt, immutable, and uniquely named**
(`buzz-agent-<first-12-hex>-<gen>`, §Pod shape). The rationale is a
concurrency race a deterministic shared Secret name cannot survive: two
concurrent deploys carrying *different* payloads would both write the shared
Secret, the loser's write could land last, and the winner's pod —
deterministic name, winner's spec — would resolve the **loser's**
identity/config through `envFrom`. The losing caller would have mutated the
winning generation despite strict no-op. Unique names close this: each
create attempt writes its own Secret first, then attempts the deterministic
pod create with a spec referencing exactly that Secret. Pod creation elects
the winner; payload and Secret are atomic at the pod-spec boundary, with no
Lease or CAS machinery.

Lifecycle rules that follow:

- **Winner**: pod + its referenced Secret live together; GC deletes them
  together.
- **Losing contender** (create-conflict): annotation-verify the winning pod,
  return its `agent_id` per the convergence rule, and delete **only its own
  now-unreferenced Secret** — never the winner's, never any Secret
  referenced by an *existing* pod. "Existing" deliberately includes
  not-yet-started pods: an `envFrom` reference from a pod still pulling its
  image is exactly as load-bearing as one from a running pod.
- **Live no-op arm**: no Secret is written at all (zero mutation).
- **GC**: also deletes annotation-verified **orphan Secrets** — those whose
  generation token no existing pod references — covering contenders that
  crashed between Secret create and their conflict cleanup. But only when
  **age-eligible**: see the normative age gate in §K8s GC — "unreferenced"
  is not "orphaned" while a concurrent attempt may still be between its
  Secret create and its pod create.

Fresh configuration therefore materializes exactly when a fresh generation
does. Residual exposure, stated: any principal with
pod-exec or secret-read in the namespace can read the nsec. This is the
substrate-security boundary from §Non-Goals — the namespace is the isolation
unit, and users deploying to shared namespaces accept its ambient RBAC. The
in-pod narrowing that sprig's dev-MCP shim performs (strips the key from its
own env, re-materializes as a 0600 keyfile for the git helpers) limits
accidental leakage into subprocess environments, not hostile cluster access.

### Garbage collection {#k8s-gc}

A **generation** is one pod-create attempt and the uniquely-named Secret it
references; the Secret's generation token is the generation's identity, and
the *current* generation is the one referenced by the existing pod's
`envFrom`.

GC is a **preflight reconciliation pass**, not a post-deploy afterthought:
on every deploy, after identity derivation and before the state transition,
the provider deletes terminated pods (and their referenced Secrets) that
match the pubkey label **and pass the full-pubkey annotation check**, plus
annotation-verified orphan Secrets whose generation token no existing pod
references (§K8s Secrets). It never touches the current generation.
Mismatched annotations are never GC'd (§Deploy State Machine step 1).

**Orphan-Secret age gate (normative).** An unreferenced Secret is
GC-eligible only when its server-assigned `creationTimestamp` is older than
**twice the deploy operation deadline** (2 × 600s). Rationale — without the
gate, GC composes with per-attempt Secrets into a legal interleaving that
strands a deploy: attempt A creates Secret A; concurrent attempt B runs its
preflight GC *before A creates its pod*, sees Secret A unreferenced, and
deletes it as an "orphan"; A's pod is then accepted referencing a missing
Secret and sits in `CreateContainerConfigError` until a later deploy
repairs it by delete-recreate (§Deploy State Machine never-started rows) —
a stranded deploy either way. Unique Secret
names made payload↔Secret atomic *at the pod-spec boundary*, but
Secret-create→pod-create is not atomic against an independent GC pass —
the standard controller lesson that observations may be stale and
reconciliation must tolerate in-flight peers. The age bound makes
"unreferenced" mean "provably abandoned": any attempt that could still
reference the Secret has exceeded its own deadline. A losing contender's
immediate cleanup of **its own** Secret is exempt — ownership, not age, is
its safety argument. (A Lease per agent identity would also close this
race; the age gate achieves the same with no extra machinery.)

**Same-clock rule (normative).** The age comparison has two operands and
both MUST come from the apiserver's clock. `creationTimestamp` is
server-assigned; the comparison instant MUST be derived from the HTTP
`Date` response header on the very list/get call the GC pass performs
(RFC 9110 §6.6.1 — origin-server message-origination time), never from the
provider's local `now()`. The provider runs on a user's desktop, and a
local clock fast by more than the margin doesn't *race* — it
deterministically computes every in-flight Secret as expired and deletes
them all, silently, on every pass, reopening exactly the interleaving the
gate exists to close. With both operands from one clock, skew cancels.
(`kube`'s `Client::send` returns the raw `http::Response` with headers, so
this costs one header read, not a departure from the typed API.) If the
`Date` header is absent or unparseable, the provider MUST **skip
orphan-Secret GC for that pass** — never fall back to local time. A
deferred cleanup is free; a wrong deletion is not.

**Alternative considered — `ownerReferences`, omitted in v1.** Kubernetes'
native GC (a Secret owned by its attempt's Pod is deleted when the owner is
verified absent) cannot *replace* the age gate: an ownerReference needs the
owner's UID, which exists only after pod create, so primary reliance on it
would flip the ordering to Pod-first-then-Secret. The reason that flip
loses is **diagnostics, not repairability**: a never-started winner is
recoverable (the kubelet retries a config-failed container indefinitely,
and the amended no-op rule lets a later deploy delete-and-recreate it with
its own payload), but Pod-first routes *every healthy deploy* through
`CreateContainerConfigError` — the exact condition the startup classifier
treats as an actionable failure signal — so the classifier could no longer
believe that reason without waiting out the deadline, on every deploy.
That trades away normative diagnostics for a cleanup the age gate already
provides. A *supplementary* post-create attachment (patch the Secret with
the winning pod's UID; Secret metadata stays patchable when `immutable` and
`data` are untouched) is sound but adds no required property: the pre-pod
crash window still needs the age-gated sweep as backstop, so v1 omits it
under the complexity budget. Any future implementation that adds it MUST
set `blockOwnerDeletion: false` explicitly (true requires `update` on
`pods/finalizers` — an RBAC verb nothing else here needs — and a Secret
should never delay its pod's deletion), MUST keep owner and dependent in
the same namespace (a cross-namespace owner is treated as *absent*, turning
the safety net into an immediate-delete instruction), and MUST treat
attachment failure as non-fatal cleanup, never a deploy error.

Running GC first
gives concurrency and Secret ownership one unambiguous order: reconcile
always observes a world with at most one candidate generation *older than
the gate*. Completed
pods from the *current* generation are left in place — their logs are the
only forensics M1 permits. That forensic window is deliberately fragile:
next-deploy GC, node loss, or namespace deletion erases it, and M1 means
there is no log operation to reach for. **Cluster-native log shipping is
therefore a production prerequisite, not an optional nicety** — the
ephemeral-runner lesson: disposable generations still need durable
diagnostics, forwarded off the pod by the cluster operator's stack. The
binding's contribution is correlation, not transport: the pod carries the
full-pubkey annotation, the generation token (doubling as
`BUZZ_MANAGED_AGENT_START_NONCE`, so lifecycle frames and pod logs share a
correlator), the provider version, and the resolved image reference
(§Image) — enough to attribute any shipped log line to an exact identity,
generation, and binary, with no secret in any of it. GC on next-deploy
also self-heals the missing
`undeploy`: delete-then-recreate converges, and a deleted-forever agent's
residue is one Completed pod that never restarts (I5) plus one Secret,
removable with `kubectl delete`.

### `provider_config` v1 fields

`context`, `namespace`, `image`, `cpu_request`, `memory_request`,
`cpu_limit`, `memory_limit`, `inactivity_seconds`, `service_account` —
9 of the 20-field validation cap. Node selectors, tolerations, and PVCs are
deliberately baked out of v1 to preserve budget.

### Distribution

Its own release workflow (macOS arm64/x64 + Linux musl; the sprig workflow's
ubuntu × musl matrix cannot produce the laptop-side binary), artifacts
attached to releases, installed to `~/.local/bin` (already on the discovery
path). v1 ships no Windows binary [DECISION B]; desktop bundling into the
.app (discovery already prepends the bundle dir) is deferred [DECISION D].

## Conformance

A provider is conforming iff:

1. `info` and `deploy` implement the wire contract (§Provider Protocol),
   including one-JSON-in/one-JSON-out, meaningful exit codes, and in-band
   `{"ok": false}` errors.
2. It never requests or accepts credentials through `provider_config` (I2).
3. It builds agent identity env from top-level payload fields, never from
   `env_vars` (reserved-key rule), applies §Launch data mechanically —
   three-tier precedence, host-resolved re-derivation, no re-merge of
   legacy `env_vars`, no provider-side model/provider mapping — and refuses
   a deploy that resolves neither `auth_tag` nor `launch.owner_pubkey`, or
   whose provider is `relay-mesh`.
4. `deploy` implements the reconciliation loop (I4): identity derived from
   the nsec before any mutation, candidates verified by full-identity
   annotation before any action, live (= **started**, §Deploy State
   Machine) → strict no-op (zero mutation), never-started states
   classified by evidence not reason strings (provably-broken →
   preconditioned delete-recreate; recoverable → observe, never delete,
   on this call or any later one), destructive reads consistent and
   destructive deletes UID+resourceVersion-preconditioned (§Deploy State
   Machine controlled-view rule), success only on confirmed container
   start, conflicts converge by re-entry, delete-not-found is success.
5. The deployed harness invocation enables an inactivity bound (I5) and the
   substrate does not resurrect terminated instances.
6. The harness is the deployed container's **signal-receiving process**, and
   the termination path delivers the termination signal to it with enough
   grace for its full graceful shutdown before force-kill (I3 staleness
   minimization). "Allows" is not enough — a wrapper that swallows the
   signal conforms to nothing.
7. It emits no secret material in any output (belt to `D`'s redaction
   suspenders).

Conformance is testable without mechanization: a fake-provider harness can
exercise items 1–3 and 7 over the wire contract — including the pre-secret
negotiation gate (§Discovery): an incompatible `protocol_version` MUST be
rejected before any request carrying `private_key_nsec` is sent, and an
executable replaced between the deploy-path `info` and `deploy` MUST fail
the same-identity check rather than receive the nsec — and an envtest/kind suite
can drive item 4's reconciler against a real apiserver — concurrent
deploys, a deletion-marked pod, terminal restart, an annotation-mismatch
collision, and SIGTERM→presence-offline for items 5–6. Two families of
cases are mandatory because they were the review-found failure modes:
**startup discrimination** (slow-but-valid scheduling → poll-then-succeed;
`Unschedulable` during scale-from-zero → observed until the autoscaler
provisions capacity, then success — never delete, **including when
provisioning completes only after the 600s deadline**: the original pod
identity and `creationTimestamp` survive the expired call and become the
no-op winner on a later deploy, the case that pins the anti-livelock rule;
referenced Secret *confirmed absent* → preconditioned delete-recreate or
actionable error, never silent success or no-op; a **never-started winner
is repairable** — pod exists, Secret absent, container never started: a
later deploy MUST delete-and-recreate rather than no-op, the test that pins
started-not-phase as the no-op criterion; and the **classification→DELETE
race** — the container transitions to running between the classifying read
and the delete: the UID+resourceVersion precondition MUST fail and the
live agent MUST be preserved) and the **GC/attempt interleaving** (attempt B's
preflight GC running between attempt A's Secret create and pod create MUST
NOT delete Secret A — the §K8s GC age gate under test; provider death after
Secret create → the age gate protects, then a later GC reaps; and a
**provider local clock fast beyond the margin** MUST NOT delete an
in-flight Secret — cheap with a fake clock, and the same-clock rule's
skip-on-absent-`Date` arm is exercised by stripping the header). A model checker is
the wrong tool here: the failure modes found in review were wrong
*abstractions of Kubernetes* (a nonexistent `Terminating` phase, non-atomic
delete, phase-as-readiness, non-atomic Secret→pod against GC), which a
hand-written model would have reproduced convincingly.

## Known Defects (at `c1bca1b56`)

Desktop- and harness-side, discovered during this design:

1. **Windows discovery id pollution**: the `.exe` suffix survives into the
   provider id, which then fails id validation at deploy — dropdown-visible,
   probe-fine, deploy-broken. Fix is a suffix strip in discovery. (v1
   provider scope is macOS+Linux regardless — [DECISION B].)
2. **Provider env inheritance**: `invoke_provider` passes the desktop's
   environment through unmodified; combined with launchd's minimal PATH this
   breaks kubeconfig exec plugins. Mitigated provider-side (§K8s Auth);
   a desktop-side PATH augmentation would fix the class.
3. **Deploy payload bypasses the launch resolver** (the prerequisite this
   spec names for §Launch data — a desktop code change, not spec text).
   At `c1bca1b56`, `deploy_payload_json` serializes raw record bytes and a
   three-layer `merged_user_env` where the local spawn uses
   `resolve_effective_harness_descriptor`'s six-layer resolution. Concrete
   consequences, each verified in review: (a) no per-runtime model/provider
   env — a remote goose agent silently ignores the user's model choice, and
   `provider_locked` runtimes would receive vars the desktop deliberately
   withholds; (b) persona-derived `agent_command` and definition-provided
   `agent_args` serialize as blank/empty — a different command line than the
   identical local agent; (c) no `owner_pubkey` — a null-`auth_tag` agent
   cannot match `!shutdown` (it *answers* it), stranding §Stop; (d) spawn
   policy (`BUZZ_ACP_RELAY_OBSERVER`, runtime `default_env` such as
   `GOOSE_MODE=auto`, team instructions, session title, lazy-pool selection)
   is absent — remote pods run different observer/approval semantics
   (`BUZZ_ACP_DEDUP`/`BUZZ_ACP_MULTIPLE_EVENT_HANDLING` are *not* on this
   list: the local writes match the harness defaults, §Launch data); (e) a
   mesh-provider agent deploys pointed at a loopback URL that cannot exist
   in the pod instead of being refused. Until `deploy_payload_json` emits
   the `launch` block, no provider can conform to §Launch data, and the
   current payload MUST be treated as insufficient for a
   semantics-preserving remote launch. **Security follow-through:** once
   secrets can arrive via `launch.env`, desktop redaction MUST collect
   candidate values from `launch.env` (and `launch.policy_env`) as well as
   legacy `agent.env_vars` — at `c1bca1b56`, `env_secrets_from_request`
   reads only `agent.env_vars` (`backend.rs`), leaving a
   definition/persona-layer secret outside the literal-value scrub.
   Conformance: a provider that echoes a launch-only secret into an error
   must come back redacted.
4. **The I5 reaper does not exist, and its natural home is a trap**
   (harness code prerequisite). `BUZZ_ACP_EXIT_AFTER_INACTIVITY` appears
   nowhere in the harness at `c1bca1b56`; §Auto-Stop is a design, not a
   description. Worse, the obvious attachment point — the existing 30s
   maintenance tick — is gated on `pool_ready` (`lib.rs:1743`), which under
   `lazy_pool` only becomes true when work arrives, so a never-mentioned
   lazy pod would never evaluate the bound: I5 dead in its most important
   case (§Auto-Stop mechanism rule). The implementation MUST run the expiry
   check on a pool-independent timer and MUST add the env var to
   `RESERVED_ENV_KEYS` in the same change.
5. **The deploy path never checks `protocol_version`** (desktop code
   prerequisite). `provider_deploy` (`backend.rs`) sends the nsec-bearing
   `deploy` request without any preceding `info` on the same resolved
   executable; §Discovery's pre-secret negotiation gate is a design, not a
   description, until the deploy command performs
   resolve-once → `info` → compatibility check → `deploy` on the same
   executable identity.

## Implementation Correspondence

| spec concept | code |
|---|---|
| Discovery, resolution rule | `desktop/src-tauri/src/managed_agents/backend.rs` (`discover_provider_candidates`, `resolve_provider_binary`) |
| Invocation, output caps, exit rule | `backend.rs` (`invoke_provider`) |
| Pre-secret negotiation gate | *to be added*: `backend.rs` deploy path — resolve-once → `info` → version check → same-identity `deploy` (Known Defect 5) |
| Redaction | `backend.rs` (`redact_secrets_with`) |
| I2 validation | `backend.rs` (`validate_provider_config`) |
| I1 refusal, payload | `desktop/src-tauri/src/commands/agents_deploy.rs` |
| Launch resolver (shared with local spawn) | `desktop/src-tauri/src/managed_agents/readiness.rs` (`resolve_effective_harness_descriptor`); `launch` block emission *to be added* to `agents_deploy.rs` (Known Defect 3) |
| Mesh rewrite (why relay-mesh is non-deployable) | `desktop/src-tauri/src/managed_agents/relay_mesh.rs`; create-time rejection in `commands/agents.rs` (`normalize_relay_mesh`) |
| Reserved-key strip | `desktop/src-tauri/src/managed_agents/env_vars.rs` (`RESERVED_ENV_KEYS`) |
| Unconditional deploy on Start | `desktop/src-tauri/src/commands/agents.rs` (`start_managed_agent`) |
| Presence publish / offline-on-exit | `crates/buzz-acp/src/lib.rs` (`publish_presence`, shutdown path) |
| `!shutdown` owner check | `crates/buzz-acp/src/lib.rs` (main loop) |
| Graceful shutdown bounded tail (~37s) | `crates/buzz-acp/src/lib.rs` (pool shutdown, then drain / presence / relay close) |
| Auto-stop flag | *to be added*: `crates/buzz-acp/src/config.rs` + a pool-independent timer (NOT the `pool_ready`-gated maintenance tick — Known Defect 4) + `RESERVED_ENV_KEYS` entry |
| Kubernetes binding | *to be added*: `crates/buzz-backend-kubernetes` |
| Sprig image | *to be added*: `Dockerfile.sprig` + workflow |

## Open Decisions

Marked `[DECISION]` inline; consolidated:

- **A. Nest scaffolding** — should the image entrypoint scaffold the agent
  workspace (AGENTS.md, RESEARCH/, …) that the desktop's `ensure_nest`
  provides locally? Recommended: yes, via a shared template crate.
- **B. Windows scope** — fix the `.exe` discovery bug in the desktop now;
  ship Windows provider binaries only on demand. Recommended as stated.
- **C. Config budget** — the 9-field v1 set above. Recommended as stated.
- **D. Desktop bundling** — `~/.local/bin` install only for v1. Recommended
  as stated.
- **E. Running-pod semantics** — no-op (recommended, both reviewers) vs
  forcible recycle on Start.
- **F. Mesh deployability** — the spec refuses relay-mesh agents
  pre-mutation in v1 (§Launch data: the transport is desktop loopback;
  serializing it fails identically but invisibly). Reviewer consensus is
  refusal; ratification requested because it makes a visible product cut
  (shared-compute agents are local-only until an in-image mesh client
  exists).
- **G. Remote override semantics** — the spec keeps local semantics: user
  env continues to beat Buzz behavior defaults remotely (three-tier
  precedence, §Launch data), because the alternative is a quiet behavior
  fork between local and remote spawns of the same record. Flagged because
  it is a policy statement about what power users may do to remote pods.
- **H. Startup budget** — with deploy success now requiring container start
  (§Deploy State Machine), the 600s operation deadline is the de facto
  cold-pull / scale-from-zero budget. The spec fixes the semantics
  narrowly: the deadline bounds how long one Start waits synchronously —
  never when anything is destroyed (recoverable startup is observational
  across calls, so a cluster whose autoscaler `new-pod-scale-up-delay`
  exceeds 600s degrades to "Start reports unconfirmed, a later Start
  adopts the now-running pod", not a livelock). The remaining SLO ruling
  is UX-only: is ten minutes of synchronous waiting the right ceiling for
  the intended cluster class?

## Summary

Remote agents extend Buzz's managed-agent model across a deliberately thin
boundary: one untrusted binary, two JSON operations, and a relay. The
desktop's obligations end at a well-formed, fail-closed deploy payload; the
provider's obligations are convergence and honesty about state; the agent's
obligation is to bound its own life. Everything else — status, control,
memory — was already on the relay, which is why the design holds: the relay
was the management plane all along.
