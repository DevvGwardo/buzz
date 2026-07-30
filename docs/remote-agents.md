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
**bounded lifetime** — and argue each from the protocol rules. As with the git
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
  connection, credential, or API by which to inspect or control `A` on `S`.
  All post-deploy observation and control flows through `R`: status is relay
  presence (kind:20001), stop is a relay message (`!shutdown`), and
  reconfiguration is a future re-deploy. This is a deliberate reduction of the
  trusted surface — `D` needs no cloud credentials, and a compromised `D`
  cannot enumerate or attack the substrate — bought at the price of the
  staleness bounds in §Presence.

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
  Enforced by the deploy state machine (§Deploy State Machine): deploy is
  keyed on the pubkey, and the Running state maps to no-op, never to
  create-a-second. Boundary: the protocol cannot prevent the same nsec being
  deployed to two different scopes (two namespaces, two clusters, or remote
  + local simultaneously) — the relay tolerates multiple connections per key,
  and preventing this would require the global registry M1 forbids. Deploying
  one key twice is user error with confusing-but-safe results (both instances
  answer), not a safety violation.

- **(I5) Bounded lifetime.** Every remote agent instance terminates: on owner
  `!shutdown`, on inactivity exceeding a configured bound, or on hard failure.
  There is no state in which an abandoned remote agent runs forever. Enforced
  *inside the harness* (the only place that can see activity, per M1) by the
  inactivity self-stop (§Auto-Stop), and made effective on the substrate by
  the binding's requirement that a terminated harness terminates its
  container (single-process pod, `restartPolicy: Never`). Boundary: I5 bounds
  *agent* lifetime, not substrate residue — a Completed pod object persists
  for forensics until the next deploy's GC (§K8s GC).

## Provider Protocol

### Discovery

`D` scans, in order: the directory containing the desktop executable, every
entry of `PATH`, and `~/.local/bin`, for executables named
`buzz-backend-<id>`. The suffix after the prefix is the provider id and MUST
match `[a-z0-9][a-z0-9_-]*`. On Windows, an `.exe`/`.bat`/`.cmd` extension
MUST be stripped before the id is derived (see §Known Defects — as of
`c1bca1b56` it is not, so Windows providers probe but cannot deploy). First
hit per filename wins. Discovery executes nothing.

**Resolution rule.** Every subsequent operation resolves the provider id
against the *current* discovery set. A stored binary path on an agent record
is a cache, revalidated against both the current candidates and the recorded
id before every use. A record edit can therefore never redirect an operation
to a binary discovery would not have found.

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
           "description": str, "config_schema": <JSON Schema>}
timeout:  10s
```

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

The agent payload (authoritative field list in
`commands/agents_deploy.rs: deploy_payload_json`):

| field | meaning |
|---|---|
| `name` | display name |
| `relay_url` | concrete WS URL (workspace fallback materialized — the remote side has no workspace notion) |
| `private_key_nsec` | **the identity** (I1: never empty) |
| `auth_tag` | NIP-OA owner attestation |
| `agent_command`, `agent_args` | the ACP agent under the harness (configurable-harness support) |
| `system_prompt`, `model`, `provider` | effective values, live-persona-first resolution |
| `turn_timeout_seconds`, `idle_timeout_seconds`, `max_turn_duration_seconds` | harness timeout knobs |
| `parallelism` | concurrent-turn bound |
| `respond_to`, `respond_to_allowlist` | inbound author gate |
| `env_vars` | merged user env: global < persona < agent |

**Reserved-key rule (normative for providers).** `D` strips
`BUZZ_PRIVATE_KEY`, `NOSTR_PRIVATE_KEY`, `BUZZ_AUTH_TAG`, `BUZZ_RELAY_URL`,
and the other reserved keys from `env_vars` before merge. A provider MUST
construct the agent environment's identity variables from the **top-level**
payload fields (`private_key_nsec` → `BUZZ_PRIVATE_KEY`/`NOSTR_PRIVATE_KEY`,
`auth_tag` → `BUZZ_AUTH_TAG`, `relay_url` → `BUZZ_RELAY_URL`); reading
`env_vars` for them yields an identityless agent.

`agent_id` is `P`'s stable handle for the deployment (the Kubernetes binding
returns the pod name). `D` stores it as `backend_agent_id`; its presence is
the `deployed` axis of I3.

**There is no `undeploy` op in v1.** Deletion of a remote agent from `D`
orphans the substrate objects; the UI therefore requires an explicit
`force_remote_delete` confirmation, and the binding's GC + I5 bound the
orphan's cost (the agent self-stops; the pod residue is reaped on the next
deploy of the same key, or manually).

### Deploy State Machine

`start` on any non-Local agent unconditionally issues `deploy` — the desktop
does not track substrate state (M1). Deploy is therefore **not** "create": it
is *converge to at-most-one-live-instance* (I4), and the provider MUST
implement it as a state machine keyed on the agent pubkey within its scope:

| observed state | action | rationale |
|---|---|---|
| no instance | create | first deploy / after GC |
| terminated (Succeeded/Failed) | delete residue, create fresh | the **normal restart path**: how a user revives a reaped or shut-down agent |
| Running | **no-op; return existing `agent_id`** | Start must never silently kill a live agent mid-turn; "already running" is the honest answer, consistent with I3 |
| starting/terminating (transitional) | return existing `agent_id` | do not race the substrate's scheduler |

**Documented consequence.** Because Running → no-op, configuration edits to a
running remote agent do not take effect until it next exits (unlike local
agents, which re-resolve on every spawn). This is an accepted v1 tradeoff;
a deliberate "recycle" affordance (stop-then-start) is the v2 path to
immediate application. [DECISION — default is no-op; owner may overrule
toward forcible recycle.]

Idempotency in the protocol sense: two `deploy`s with the same payload
against any state converge to one live instance and return an `agent_id`;
no sequence of `deploy`s can yield two.

### Stop and Delete

- **Stop** is not a provider operation. `D` publishes `!shutdown` mentioning
  the agent on `R`; the harness verifies the sender is the owner and exits
  through its graceful path (drain in-flight turns ≤30s, publish presence
  `offline` ≤2s, close relay connection ≤5s — ~37s worst case; §K8s Grace
  sizes for this). The desktop's local stop command rejects remote agents.
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
  Remote providers opt in (the Kubernetes binding sets 7200 = 2h).
- **"Inactivity" is defined as**: no events dispatched to the agent and no
  turns in flight. Raw relay traffic does not count — an agent lurking in a
  busy channel it never answers is exactly the waste this bounds.
- **Mechanism**: the harness's existing 30s maintenance tick checks
  last-activity against the bound and, on expiry, fires the same shutdown
  channel `!shutdown` uses — so inactivity exit gets in-flight drain,
  presence→offline, and graceful relay close identically to an owner stop.
  Granularity: the tick makes the effective bound `t ∈ [T, T+30s)`, which is
  immaterial at T=7200.
- Distinctness note: this is a **fourth** timeout concept, deliberately named
  away from the existing three (`--idle-timeout` = per-turn ACP wire silence,
  900s; `turn_timeout`; `max_turn_duration` = 7200s — numerically equal to
  the default inactivity bound and semantically unrelated). Sharing a flag or
  env name with any of them is how the bug ships.

The harness exiting MUST terminate the container (the harness is PID 1 or
the sole supervised process), which with `restartPolicy: Never` completes the
pod — turning agent-level I5 into substrate-level I5.

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
system gitconfig wiring the nostr signing and credential helpers. ~15–25MB;
not FROM-scratch (bash and git preclude it). Sprig-only: alternate-harness
dependencies (node for Claude Code / Codex) come via the `image` override
field, not a fatter default. Tagging follows the relay image's matrix —
`sha-<short>` on main, semver on `sprig-v*` tags (the sprig tarball's
`+git.<sha>` version string is not a legal Docker tag). The provider bakes
its build git-sha at compile time and defaults `image` to
`ghcr.io/block/buzz-sprig:sha-<that>`, so provider and image derive from one
commit; `image` accepts tag, digest, or full custom registry reference.

### Pod shape

- **Bare Pod, `restartPolicy: Never`.** Eviction → presence `offline` (I3)
  → user hits Start → state machine's terminated arm re-creates. No Job, no
  controller: a restart controller would resurrect what `!shutdown` and
  auto-stop terminate, violating I5.
- **Naming/labeling** (63-char label-value limit; hex pubkey is 64):
  - pod name: `buzz-agent-<first-12-hex>`
  - label `buzz-agent-pubkey: <first-32-hex>` — the selector key for the
    state machine and GC (128 bits, collision-free at any plausible scale)
  - annotation carrying the **full** pubkey (annotations allow 256KB)
- **`terminationGracePeriodSeconds: 60`.** The harness's graceful shutdown is
  ~37s worst case (30s drain + 2s presence + 5s relay close); Kubernetes'
  default 30s grace would SIGKILL it mid-drain, leaving presence stale-online
  — the avoidable half of I3's staleness window. 60s covers it with margin.
- **Resources**: requests 1 cpu / 2Gi, limits 2 cpu / 4Gi, all four
  configurable (`cargo build` in an agent workspace makes 500m/1Gi requests
  unrealistic).
- **Workspace**: `emptyDir`. Checkouts and scratch die with the pod; agent
  memory is relay-persisted (NIP-AE) and unaffected. PVC support is a
  deferred knob. [DECISION A — whether the image entrypoint scaffolds the
  nest workspace (AGENTS.md etc.), which local agents get from the desktop's
  `ensure_nest` and remote pods currently would not.]

### Secrets {#k8s-secrets}

Per-agent `Secret` named for the pod, containing the identity variables
(built from top-level payload fields per the reserved-key rule) plus
`env_vars`; consumed via `envFrom`; replaced atomically on re-deploy;
deleted by GC with its pod. Residual exposure, stated: any principal with
pod-exec or secret-read in the namespace can read the nsec. This is the
substrate-security boundary from §Non-Goals — the namespace is the isolation
unit, and users deploying to shared namespaces accept its ambient RBAC. The
in-pod narrowing that sprig's dev-MCP shim performs (strips the key from its
own env, re-materializes as a 0600 keyfile for the git helpers) limits
accidental leakage into subprocess environments, not hostile cluster access.

### Garbage collection {#k8s-gc}

On every deploy, after the state machine acts, the provider deletes
terminated pods (and their Secrets) matching the pubkey label other than the
one just created/observed. Completed pods from the *current* generation are
left in place — their logs are the only forensics M1 permits. GC on
next-deploy also self-heals the missing `undeploy`: delete-then-recreate
converges, and a deleted-forever agent's residue is one Completed pod that
never restarts (I5) plus one Secret, removable with `kubectl delete`.

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
   `env_vars` (reserved-key rule).
4. `deploy` implements the convergence state machine (I4), including
   Running → no-op.
5. The deployed harness invocation enables an inactivity bound (I5) and the
   substrate does not resurrect terminated instances.
6. Its termination path allows the harness's full graceful shutdown before
   force-kill (I3 staleness minimization).
7. It emits no secret material in any output (belt to `D`'s redaction
   suspenders).

## Known Defects (at `c1bca1b56`)

Desktop-side, discovered during this design; both predate it:

1. **Windows discovery id pollution**: the `.exe` suffix survives into the
   provider id, which then fails id validation at deploy — dropdown-visible,
   probe-fine, deploy-broken. Fix is a suffix strip in discovery. (v1
   provider scope is macOS+Linux regardless — [DECISION B].)
2. **Provider env inheritance**: `invoke_provider` passes the desktop's
   environment through unmodified; combined with launchd's minimal PATH this
   breaks kubeconfig exec plugins. Mitigated provider-side (§K8s Auth);
   a desktop-side PATH augmentation would fix the class.

## Implementation Correspondence

| spec concept | code |
|---|---|
| Discovery, resolution rule | `desktop/src-tauri/src/managed_agents/backend.rs` (`discover_provider_candidates`, `resolve_provider_binary`) |
| Invocation, output caps, exit rule | `backend.rs` (`invoke_provider`) |
| Redaction | `backend.rs` (`redact_secrets_with`) |
| I2 validation | `backend.rs` (`validate_provider_config`) |
| I1 refusal, payload | `desktop/src-tauri/src/commands/agents_deploy.rs` |
| Reserved-key strip | `desktop/src-tauri/src/managed_agents/env_vars.rs` (`RESERVED_ENV_KEYS`) |
| Unconditional deploy on Start | `desktop/src-tauri/src/commands/agents.rs` (`start_managed_agent`) |
| Presence publish / offline-on-exit | `crates/buzz-acp/src/lib.rs` (`publish_presence`, shutdown path) |
| `!shutdown` owner check | `crates/buzz-acp/src/lib.rs` (main loop) |
| Graceful shutdown budget (~37s) | `crates/buzz-acp/src/lib.rs` (drain / presence / relay close) |
| Auto-stop flag | *to be added*: `crates/buzz-acp/src/config.rs` + maintenance tick |
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

## Summary

Remote agents extend Buzz's managed-agent model across a deliberately thin
boundary: one untrusted binary, two JSON operations, and a relay. The
desktop's obligations end at a well-formed, fail-closed deploy payload; the
provider's obligations are convergence and honesty about state; the agent's
obligation is to bound its own life. Everything else — status, control,
memory — was already on the relay, which is why the design holds: the relay
was the management plane all along.
