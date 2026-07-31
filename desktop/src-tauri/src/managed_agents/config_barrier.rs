//! The boot config barrier: gather every coordinate, decide, enforce.
//!
//! This is the entry point that makes [`super::decision::decide`] mean
//! something. It runs once per boot, before the flush loop can publish, and
//! wires the three layers together in the order V3.4 requires:
//!
//! 1. enumerate the coordinates this device knows about, from disk;
//! 2. run every relay lookup with NO store lock held;
//! 3. take `managed_agents_store_lock` once, re-check the scope, gather the
//!    local half, decide, and enforce — all under that single guard.
//!
//! # Why the barrier exists
//!
//! Without it, boot re-signs local disk content at
//! `monotonic_created_at = max(now, head + 1)` and queues it for publish. A
//! store that is merely stale therefore mints events that are strictly newer
//! than the edits they revert, and the relay's last-write-wins resolution
//! adopts the revert. The barrier's job is to run BEFORE that publish and
//! decide, per coordinate, whether the local content is an edit worth
//! publishing or a stale copy that must be suppressed.
//!
//! # What it enforces, and what it does not
//!
//! It enforces exactly one thing: the publication gate
//! ([`super::config_sync::apply_gate`]). A coordinate whose decision blocks
//! publication is withheld from [`super::retention::get_pending_sync`] and
//! cannot reach the relay on this boot. Decisions that PATCH disk
//! (`ApplyHead`, `RestoreFromRelay`, `DeleteLocal`) are logged and gated but
//! not applied here — applying them is the resolution path (V3.6), which is
//! user-driven by design. Suppression is the half that stops the data loss;
//! adopting remote content is the half that needs a human.

use std::collections::BTreeSet;

use tauri::Manager;

use super::{
    canonical_projection::CanonicalProjection,
    config_sync::{local_state, run_decision_pass, CoordinateState},
    decision::HeadState,
    head_lookup::{fetch_head, Coordinate},
    retention::{active_retention_scope, open_retention_db, pending_coordinates, RetentionScope},
    tombstone_scan::scan_tombstones,
};
use crate::app_state::AppState;
use buzz_core_pkg::kind::{KIND_MANAGED_AGENT, KIND_PERSONA, KIND_TEAM};

/// Run the boot barrier for the active scope.
///
/// Attempts to claim the `Unready → InProgress` transition. If another
/// caller already owns it (state is `InProgress` or `Ready`), this returns
/// immediately — the in-flight or completed barrier is sufficient.
///
/// On success, marks the scope `Ready`, allowing the flush loop to publish
/// for that scope. On any error, resets to `Unready` so the scope stays
/// fail-closed until a later retry succeeds.
///
/// Called from the flush loop (retry path, only when `Unready`). The
/// `claim_in_progress` CAS ensures at most one barrier runs at a time.
/// `spawn_event_sync` calls [`run_boot_barrier_after_claim`] instead, which
/// assumes the caller already holds `InProgress` and skips the CAS.
pub async fn run_boot_barrier(app: &tauri::AppHandle) {
    use super::config_sync_readiness;

    // Single-flight: only the first caller to claim InProgress runs the
    // barrier. If the state is already InProgress (spawn_event_sync's
    // reconcile is in-flight) or Ready, skip.
    let Some(claim) = config_sync_readiness::claim_in_progress() else {
        return;
    };

    run_boot_barrier_enforcing(app, claim).await;
}

/// Execute the barrier enforcement assuming the caller already claimed
/// `InProgress`.
///
/// Called from `spawn_event_sync` after reconcile completes, and from
/// `apply_workspace` (via `spawn_event_sync_with_held_claim`) after legacy
/// migration + reconcile. The caller holds a [`ReadinessClaim`] through the
/// entire reconcile+barrier window, preventing any concurrent flush tick from
/// running its own barrier against the pre-reconcile database state.
///
/// On success: transitions to `Ready(db_path)`. On any error: resets to
/// `Unready` (fail-closed, retryable on the next flush tick).
pub(crate) async fn run_boot_barrier_after_claim(
    app: &tauri::AppHandle,
    claim: super::config_sync_readiness::ReadinessClaim,
) {
    run_boot_barrier_enforcing(app, claim).await;
}

/// Shared enforcement body: scope resolution, relay lookups, decision pass.
///
/// Takes ownership of the `ReadinessClaim` RAII guard. On the success path,
/// the guard is resolved before `mark_ready` sets the final state. On any
/// error path (including panics), the guard's `Drop` impl resets to `Unready`
/// — so a panic inside `run_boot_barrier_for_scope` can never leave the latch
/// permanently wedged at `InProgress`.
async fn run_boot_barrier_enforcing(
    app: &tauri::AppHandle,
    claim: super::config_sync_readiness::ReadinessClaim,
) {
    use super::config_sync_readiness;

    let state = app.state::<AppState>();
    let scope = match active_retention_scope(app, &state) {
        Ok(scope) => scope,
        Err(error) => {
            tracing::warn!(target: "buzz::config_sync", %error, "boot barrier skipped: no active scope");
            // claim drops here → mark_unready via RAII
            return;
        }
    };

    match run_boot_barrier_for_scope(app, &state, &scope).await {
        Ok(()) => {
            // Resolve the claim so the RAII drop does not reset to Unready.
            claim.resolve();
            config_sync_readiness::mark_ready(scope.db_path.clone());
            tracing::info!(
                target: "buzz::config_sync",
                db_path = %scope.db_path.display(),
                "boot barrier complete; scope is ready for publication"
            );
        }
        Err(error) => {
            tracing::warn!(target: "buzz::config_sync", %error, "boot barrier failed; scope stays closed for retry");
            // claim drops here → mark_unready via RAII (explicit call would
            // double-reset, but that is idempotent; RAII is the safety net).
        }
    }
}

/// Withhold every queued publish that has no baseline, before any network I/O.
///
/// This is V3.9's legacy-pending quarantine, and it is also what makes the
/// barrier safe against its own latency. The barrier's decisions need a
/// per-coordinate relay lookup plus a full tombstone scan; the flush loop needs
/// only a timer. On a slow network the flush can therefore win the race and
/// publish exactly the rows the barrier was about to gate.
///
/// Quarantining first inverts that: the gate is closed synchronously, before
/// the first round trip, and the decision pass re-opens it for the coordinates
/// that earn it. A row is either provably publishable or it is not going out.
///
/// Note: `disk_projections` runs before this call inside
/// `run_boot_barrier_for_scope`. Under the readiness latch that ordering is
/// defense-in-depth — the flush loop cannot snapshot any scope that has not
/// yet passed its barrier, so quarantine ordering no longer determines whether
/// a pre-quarantine row can escape. The synchronous-before-network property is
/// retained because it is cheap and keeps the invariant obvious.
///
/// Scoped to rows with no baseline because those are precisely the unprovable
/// ones: a legacy row queued before the baseline column existed, and a row
/// queued by a second store whose retention database has never agreed to
/// anything at that coordinate. A row WITH a baseline is left alone — its
/// arbitration is decidable, and closing its gate here would stall an ordinary
/// edit behind a scan it does not need.
///
/// Returns how many rows it withheld.
fn quarantine_unprovable_pending(
    conn: &rusqlite::Connection,
    owner_pubkey: &str,
) -> Result<usize, String> {
    conn.execute(
        "UPDATE persona_events SET publish_blocked = 1
         WHERE pending_sync = 1 AND pubkey = ?1 AND baseline_event_id IS NULL",
        rusqlite::params![owner_pubkey],
    )
    .map_err(|error| format!("failed to quarantine unprovable pending rows: {error}"))
}

async fn run_boot_barrier_for_scope(
    app: &tauri::AppHandle,
    state: &AppState,
    scope: &RetentionScope,
) -> Result<(), String> {
    let owner_pubkey = scope.owner_keys.public_key().to_hex();

    // Enumerate BEFORE the lookups: the set of coordinates to decide is the
    // union of what disk holds and what is already queued for publish. A
    // pending row can outlive its record (deleted persona, removed agent, or a
    // tombstone), and those rows publish with nothing on disk to enumerate
    // them — exactly the ones that most need gating. The gate must see
    // everything the publisher can see, and the publisher reads the table.
    let mut disk = disk_projections(app)?;
    {
        let conn = open_retention_db(&scope.db_path)?;
        let quarantined = quarantine_unprovable_pending(&conn, &owner_pubkey)?;
        if quarantined > 0 {
            tracing::info!(
                target: "buzz::config_sync",
                quarantined,
                "withheld pending rows with no baseline pending the decision pass"
            );
        }

        let known: BTreeSet<(u32, String)> = disk
            .iter()
            .map(|(coordinate, _)| (coordinate.kind, coordinate.d_tag.clone()))
            .collect();
        for (kind, d_tag) in pending_coordinates(&conn, &owner_pubkey)? {
            if !known.contains(&(kind, d_tag.clone())) {
                // No disk record: the projection is `None`, which is what the
                // table reads as "absent locally".
                disk.push((Coordinate { kind, d_tag }, None));
            }
        }
    }

    // Phase 1 — relay lookups, no lock held. Each head read is an exact
    // per-coordinate query; a failure becomes `LookupFailed`
    // rather than an absence, so a flaky network can never be read as a
    // deletion. The tombstone scan runs once for the whole owner.
    let api_base_url = crate::relay::relay_http_base_url(&scope.relay_url);
    let mut heads: Vec<(Coordinate, HeadState)> = Vec::with_capacity(disk.len());
    for (coordinate, _) in &disk {
        heads.push((
            coordinate.clone(),
            fetch_head(state, scope, &api_base_url, coordinate).await,
        ));
    }
    let tombstones = scan_tombstones(state, scope, &api_base_url).await;

    // Phase 2 — one guard for the whole decision pass.
    let _store_guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;

    // Phase 3 — re-resolve the scope under the lock. `apply_workspace` can swap
    // the relay and keys mid-flight without taking this lock, so without this
    // check a completed pull from community A could patch community B's store.
    let current = active_retention_scope(app, state)?;
    if current.db_path != scope.db_path {
        tracing::info!(
            target: "buzz::config_sync",
            "boot barrier abandoned: workspace switched during relay lookups"
        );
        return Ok(());
    }

    let conn = open_retention_db(&scope.db_path)?;
    let mut states: Vec<CoordinateState> = Vec::with_capacity(disk.len());
    for ((coordinate, projection), (_, head)) in disk.into_iter().zip(heads) {
        let tombstone = tombstones.evidence(&coordinate);
        let observation = local_state(
            &conn,
            &owner_pubkey,
            &coordinate,
            projection,
            head,
            tombstone,
        )?;
        states.push(CoordinateState {
            coordinate,
            observation,
        });
    }

    run_decision_pass(&conn, &owner_pubkey, &states)?;
    Ok(())
}

/// Read the three record stores and enumerate their coordinates.
fn disk_projections(
    app: &tauri::AppHandle,
) -> Result<Vec<(Coordinate, Option<CanonicalProjection>)>, String> {
    use super::{load_managed_agents, load_teams, personas::load_personas};

    Ok(project_records(
        &load_personas(app)?,
        &load_teams(app)?,
        &load_managed_agents(app)?,
    ))
}

/// Every config coordinate the given records occupy, with what each would
/// publish.
///
/// Uses the same d-tag derivations the publish paths use, so a coordinate can
/// never be enumerated under a key that differs from the one it would publish
/// under. Built-in records are excluded: they come from code, are never
/// published, and have no relay head to compare against.
///
/// A record whose projection cannot be built is skipped with a warning rather
/// than failing the pass — one malformed record must not stop the barrier from
/// protecting every other coordinate.
fn project_records(
    personas: &[super::AgentDefinition],
    teams: &[super::TeamRecord],
    agents: &[super::ManagedAgentRecord],
) -> Vec<(Coordinate, Option<CanonicalProjection>)> {
    let mut out: Vec<(Coordinate, Option<CanonicalProjection>)> = Vec::new();
    let mut seen: BTreeSet<(u32, String)> = BTreeSet::new();

    let mut push = |kind: u32, d_tag: String, projection: Result<CanonicalProjection, String>| {
        match projection {
            Ok(projection) => {
                // Two records can normalize onto one d-tag. Deciding the
                // coordinate twice would gate it twice from the same inputs,
                // so the first wins and the collision is visible in the log.
                if seen.insert((kind, d_tag.clone())) {
                    out.push((Coordinate { kind, d_tag }, Some(projection)));
                } else {
                    tracing::warn!(
                        target: "buzz::config_sync",
                        kind,
                        d_tag = %d_tag,
                        "duplicate coordinate on disk; deciding the first record only"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(
                    target: "buzz::config_sync",
                    kind,
                    d_tag = %d_tag,
                    %error,
                    "skipping coordinate with an unbuildable projection"
                );
            }
        }
    };

    for record in personas.iter().filter(|record| !record.is_builtin) {
        push(
            KIND_PERSONA,
            super::persona_events::persona_d_tag(record),
            CanonicalProjection::for_persona(record),
        );
    }
    for record in teams.iter().filter(|record| !record.is_builtin) {
        push(
            KIND_TEAM,
            record.id.clone(),
            CanonicalProjection::for_team(record),
        );
    }
    for record in agents
        .iter()
        // A key-less agent has no coordinate yet: its d-tag IS its pubkey, and
        // it mints one on first start.
        .filter(|record| !record.pubkey.is_empty())
    {
        push(
            KIND_MANAGED_AGENT,
            record.pubkey.clone(),
            CanonicalProjection::for_managed_agent(record),
        );
    }

    out
}

#[cfg(test)]
mod tests;
