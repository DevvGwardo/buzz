//! Boot config sync: gather each coordinate's observable state, decide, enforce.
//!
//! This is the layer between the pure decision table
//! ([`super::decision::decide`]) and the stores it reads. It owns two things
//! the table deliberately does not: WHERE each input comes from, and the
//! ordering guarantees around reading them.
//!
//! # Ordering (V3.4)
//!
//! No network I/O may happen while the store lock is held, and no decision may
//! be made from inputs gathered under a different workspace:
//!
//! 1. capture `(relay, owner, db path)` as a scope snapshot;
//! 2. run the relay lookups with NO lock held;
//! 3. take `managed_agents_store_lock` for the whole decision pass;
//! 4. re-resolve the active scope and abandon the pass if it moved;
//! 5. evaluate the table and write, still under that one guard.
//!
//! Step 4 is load-bearing rather than defensive: `apply_workspace` can swap the
//! relay and keys mid-flight and does not take this lock, so without the
//! recheck a completed pull from community A could patch community B's shared
//! disk store.

use super::{
    canonical_projection::CanonicalProjection,
    decision::{Decision, Head, HeadState, Observation, Resolution, TombstoneEvidence},
    head_lookup::Coordinate,
    retention::{get_baseline, get_retained_event, RetainedEvent},
};
use rusqlite::Connection;

/// Everything the decision table needs about one coordinate, plus the identity
/// of the coordinate itself.
#[derive(Debug, Clone)]
pub struct CoordinateState {
    pub coordinate: Coordinate,
    pub observation: Observation,
}

/// Read the locally-known half of a coordinate's state: the queued publish, any
/// queued deletion, and the baseline.
///
/// The head and the tombstone evidence come from the relay and are supplied by
/// the caller, which has already run those lookups outside the lock.
///
/// The coordinate's own row and its paired tombstone row occupy different
/// primary keys and are reported separately, because the table resolves them on
/// different evidence — a deletion by timestamp against the head, an edit
/// against the baseline. Collapsing them into one flag was the cross-coordinate
/// hole called out in review; reporting only the live row is the same hole.
pub fn local_state(
    conn: &Connection,
    owner_pubkey: &str,
    coordinate: &Coordinate,
    disk: Option<CanonicalProjection>,
    head: HeadState,
    tombstone: TombstoneEvidence,
) -> Result<Observation, String> {
    let row = get_retained_event(conn, coordinate.kind, owner_pubkey, &coordinate.d_tag)?;
    let tombstone_row = get_retained_event(
        conn,
        buzz_core_pkg::kind::KIND_DELETION,
        owner_pubkey,
        &super::retention::tombstone_retention_d_tag(coordinate.kind, &coordinate.d_tag),
    )?;

    Ok(Observation {
        disk,
        queued: queued_projection(row.as_ref()),
        head,
        baseline: baseline_projection(conn, owner_pubkey, coordinate, row.as_ref())?,
        queued_deletion_at: tombstone_row
            .as_ref()
            .filter(|row| row.pending_sync)
            .map(|row| row.created_at),
        tombstone,
    })
}

/// What a coordinate's queued row would publish, or `None` when nothing is
/// queued there.
///
/// Built from the retained row's own `raw_event` rather than from disk: the
/// flush loop publishes exactly those bytes, so those are the bytes the
/// baseline arbitration has to be about. A row whose stored event will not
/// parse reports `None` — it cannot publish either, since the flush loop parses
/// the same field and fails the row.
fn queued_projection(row: Option<&RetainedEvent>) -> Option<CanonicalProjection> {
    use nostr::JsonUtil;
    let row = row.filter(|row| row.pending_sync)?;
    let event = nostr::Event::from_json(&row.raw_event).ok()?;
    Some(CanonicalProjection::from_event(&event))
}

/// The baseline projection for a coordinate, or `None` when this install has
/// no provenance for it.
///
/// The stored baseline is the CONTENT that was published; the `shared` half of
/// the projection is recovered from the baseline event's own tags via the
/// retained row, so a baseline comparison sees the same two fields a head
/// comparison does.
fn baseline_projection(
    conn: &Connection,
    owner_pubkey: &str,
    coordinate: &Coordinate,
    row: Option<&RetainedEvent>,
) -> Result<Option<CanonicalProjection>, String> {
    let Some((baseline_event_id, baseline_content)) =
        get_baseline(conn, coordinate.kind, owner_pubkey, &coordinate.d_tag)?
    else {
        return Ok(None);
    };

    // Recover `shared` from the retained event when the row still holds the
    // baseline event; otherwise the tag state is unknown and the baseline is
    // only usable for kinds that never carry one.
    let shared = row
        .filter(|row| row.event_id.as_deref() == Some(baseline_event_id.as_str()))
        .and_then(|row| {
            use nostr::JsonUtil;
            nostr::Event::from_json(&row.raw_event).ok()
        })
        .map(|event| CanonicalProjection::from_event(&event).shared)
        .unwrap_or(false);

    Ok(Some(CanonicalProjection {
        content: baseline_content,
        shared,
    }))
}

/// Resolve every gathered coordinate, pairing each resolution with its target.
///
/// Kept separate from execution so a caller can log or inspect the full plan
/// before anything is written — and so the mapping is testable end to end
/// without a relay or a disk store.
pub fn plan(states: &[CoordinateState]) -> Vec<(Coordinate, Resolution)> {
    states
        .iter()
        .map(|state| {
            (
                state.coordinate.clone(),
                super::decision::decide(&state.observation),
            )
        })
        .collect()
}

/// Whether a resolution must withhold the coordinate's LIVE row from the flush
/// loop.
///
/// The gate is DEFAULT-DENY over the decision set: it lists what may publish,
/// not what may not. Every publishing decision names itself here, so a variant
/// added later is withheld until someone deliberately admits it — the opposite
/// default would let a new decision reach the relay by omission.
///
/// Clearing the gate is as load-bearing as setting it: a coordinate suppressed
/// on one boot must resume publishing once the condition that suppressed it is
/// gone, or the gate latches shut permanently and the record strands.
pub fn blocks_publication(resolution: &Resolution) -> bool {
    match resolution.decision {
        // The one decision that publishes local content.
        Decision::PublishLocalEdit => false,
        // The deletion publishes from the TOMBSTONE row, not this one. The
        // live row is the thing being deleted, so it must stay withheld:
        // publishing it would race its own tombstone at the same coordinate.
        Decision::PublishDeletion => true,
        // Adopts the relay's version onto disk, so nothing local is going out.
        // Only reachable with a baseline present and nothing queued (gate D
        // takes a queued row first), so releasing strands nothing.
        Decision::ApplyHead => false,
        // Only reachable with NO local record and nothing queued, so there is
        // nothing to withhold — and nothing to strand by withholding. Blocked
        // rather than released because it is the one adopt-the-relay cell
        // reachable with no baseline, and a coordinate with no provenance
        // against a live head must not leave an open gate behind for a row
        // queued after the pass.
        Decision::RestoreFromRelay => true,
        // `StampBaseline` is easy to miss: disk already equals the head, so
        // publishing would push byte-identical content and burn a
        // `monotonic_created_at` bump for nothing.
        Decision::StampBaseline
        | Decision::SuppressPublish
        | Decision::Park(_)
        | Decision::DeleteLocal
        | Decision::Defer => true,
    }
}

/// Whether a resolution must withhold the coordinate's queued TOMBSTONE row.
///
/// A queued deletion does not live at the coordinate it deletes: it is retained
/// at `(5, owner, "<target_kind>:<target_d_tag>")`, a different primary key.
/// Gating only [`blocks_publication`]'s row would therefore compute a
/// suppression and enforce it on the wrong row, leaving the tombstone free to
/// publish — the exact "decorative gate" failure this pass exists to avoid, and
/// the worse direction of it: a deletion that escapes the gate destroys a head
/// rather than merely reverting it.
///
/// Exactly one decision releases it. Every other outcome — including the
/// timestamp arbitration falling through to the live cells — means the queued
/// deletion did not win, so it stays withheld.
pub fn blocks_tombstone_publication(resolution: &Resolution) -> bool {
    resolution.decision != Decision::PublishDeletion
}

/// Log one resolution with the inputs that produced it, then enforce BOTH
/// publication gates.
///
/// Two rows, not one: the coordinate's live row and its queued tombstone row
/// occupy different primary keys, and the flush loop reads them independently.
/// A single `set_publish_blocked` call therefore enforces half a decision.
///
/// The log line is a hard requirement, not debug output: this pass silently
/// changes what a device publishes, and the boot log is the only evidence of
/// why a coordinate did or did not go out. Every field the table branched on is
/// recorded, so a surprising decision can be re-derived from the log alone
/// without reproducing the machine's store.
pub fn apply_gate(
    conn: &Connection,
    owner_pubkey: &str,
    coordinate: &Coordinate,
    observation: &Observation,
    resolution: &Resolution,
) -> Result<(), String> {
    tracing::info!(
        target: "buzz::config_sync",
        kind = coordinate.kind,
        d_tag = %coordinate.d_tag,
        decision = ?resolution.decision,
        blocked = blocks_publication(resolution),
        tombstone_blocked = blocks_tombstone_publication(resolution),
        cleared_queued_publish = resolution.clear_queued_publish,
        head = match &observation.head {
            HeadState::Present(_) => "present",
            HeadState::Absent => "absent",
            HeadState::LookupFailed => "lookup_failed",
        },
        disk_present = observation.disk.is_some(),
        disk_matches_head = matches!(
            (&observation.disk, &observation.head),
            (Some(disk), HeadState::Present(head)) if head.projection == *disk
        ),
        has_baseline = observation.baseline.is_some(),
        queued = observation.queued.is_some(),
        queued_matches_baseline = matches!(
            (&observation.queued, &observation.baseline),
            (Some(queued), Some(baseline)) if queued == baseline
        ),
        queued_deletion = observation.queued_deletion_at.is_some(),
        tombstone = ?observation.tombstone,
        "config-sync decision"
    );

    super::retention::set_publish_blocked(
        conn,
        coordinate.kind,
        owner_pubkey,
        &coordinate.d_tag,
        blocks_publication(resolution),
    )?;

    super::retention::set_publish_blocked(
        conn,
        buzz_core_pkg::kind::KIND_DELETION,
        owner_pubkey,
        &super::retention::tombstone_retention_d_tag(coordinate.kind, &coordinate.d_tag),
        blocks_tombstone_publication(resolution),
    )
}

/// Record the relay head as this install's baseline for a coordinate.
///
/// Only ever called with a head that a completed, exact, best-available
/// (replica-lag-exposed) lookup
/// actually returned, which is what makes the baseline a claim about the RELAY
/// rather than about local disk. Stamping from disk content would defeat the
/// entire model: the baseline's job is to distinguish "the user edited this"
/// from "this store never learned about the newer head", and content that was
/// never published cannot answer that question.
fn stamp_baseline_from_head(
    conn: &Connection,
    owner_pubkey: &str,
    coordinate: &Coordinate,
    head: &Head,
) -> Result<(), String> {
    super::retention::set_baseline(
        conn,
        coordinate.kind,
        owner_pubkey,
        &coordinate.d_tag,
        &head.event_id,
        &head.projection.content,
    )
}

/// Run the boot decision pass for a set of coordinates and enforce the result.
///
/// This is the barrier's entry point. Ordering follows V3.4 strictly: the
/// caller performs every relay lookup BEFORE calling this, and this function
/// does only local work, so no network I/O ever happens while the store lock is
/// held.
///
/// Three outcomes are executed here, and only three: the publication gate, the
/// baseline stamp, and dropping a vacuous queued publish. All three are
/// provenance — none touches the user's records. The decisions that PATCH disk
/// (`ApplyHead`, `RestoreFromRelay`, `DeleteLocal`) are logged and gated but
/// deliberately not applied, because adopting remote content over a local
/// record is the resolution path (V3.6) and is user-driven by design.
///
/// Returns the plan it enforced, so the caller can act on the non-gate half of
/// each resolution and log a summary.
pub fn run_decision_pass(
    conn: &Connection,
    owner_pubkey: &str,
    states: &[CoordinateState],
) -> Result<Vec<(Coordinate, Resolution)>, String> {
    let plan = plan(states);

    for (state, (coordinate, resolution)) in states.iter().zip(plan.iter()) {
        apply_gate(
            conn,
            owner_pubkey,
            coordinate,
            &state.observation,
            resolution,
        )?;

        // A vacuous queued publish is dropped rather than gated. Gating would
        // hold it forever: it can never become publishable, because it says
        // nothing the baseline does not already say.
        if resolution.clear_queued_publish {
            super::retention::clear_pending_sync(
                conn,
                coordinate.kind,
                owner_pubkey,
                &coordinate.d_tag,
            )?;
        }

        // `StampBaseline` means disk and head already agree, so the head is a
        // sound baseline and recording it is what gives every later boot the
        // provenance to tell an edit from a stale copy. Without this the
        // baseline stays `None` forever and every baseline-dependent cell is
        // unreachable.
        if resolution.decision == Decision::StampBaseline {
            if let HeadState::Present(head) = &state.observation.head {
                stamp_baseline_from_head(conn, owner_pubkey, coordinate, head)?;
            }
        }
    }

    let blocked = plan
        .iter()
        .filter(|(_, resolution)| blocks_publication(resolution))
        .count();
    let cleared = plan
        .iter()
        .filter(|(_, resolution)| resolution.clear_queued_publish)
        .count();
    tracing::info!(
        target: "buzz::config_sync",
        coordinates = plan.len(),
        blocked,
        cleared,
        "config-sync decision pass complete"
    );

    Ok(plan)
}

#[cfg(test)]
mod tests;
