use std::path::Path;

use super::*;
use crate::managed_agents::{
    decision::{decide, ParkReason},
    persona_events::build_persona_event,
    retention::{
        get_baseline, get_pending_sync, get_retained_event, is_publish_blocked, open_retention_db,
        retain_event, set_baseline, tombstone_retention_d_tag, RetainedEvent,
    },
};
use buzz_core_pkg::kind::{KIND_DELETION, KIND_PERSONA};

const OWNER: &str = "ownerpubkeyhex";

fn test_db() -> Connection {
    open_retention_db(Path::new(":memory:")).unwrap()
}

fn coordinate() -> Coordinate {
    Coordinate {
        kind: KIND_PERSONA,
        d_tag: "test-persona".to_string(),
    }
}

/// The retention key the coordinate's queued tombstone occupies — a DIFFERENT
/// primary key from the coordinate itself, which is the whole reason the gate
/// has two halves.
fn tombstone_key() -> String {
    tombstone_retention_d_tag(KIND_PERSONA, "test-persona")
}

fn projection(content: &str) -> CanonicalProjection {
    CanonicalProjection {
        content: content.to_string(),
        shared: false,
    }
}

/// A relay head carrying `content` at `created_at`, with an id derived from it.
fn head(content: &str, created_at: i64) -> Head {
    Head {
        event_id: format!("id-{content}"),
        created_at,
        projection: projection(content),
    }
}

fn signed_persona(system_prompt: &str) -> nostr::Event {
    use std::collections::BTreeMap;
    let record = crate::managed_agents::AgentDefinition {
        id: "test-persona".to_string(),
        display_name: "Test".to_string(),
        avatar_url: None,
        system_prompt: system_prompt.to_string(),
        runtime: None,
        model: None,
        provider: None,
        name_pool: Vec::new(),
        is_builtin: false,
        is_active: true,
        shared: false,
        source_team: None,
        source_team_persona_slug: None,
        catalog_source: None,
        env_vars: BTreeMap::new(),
        respond_to: None,
        respond_to_allowlist: Vec::new(),
        parallelism: None,
        created_at: "2025-01-01T00:00:00Z".to_string(),
        updated_at: "2025-01-01T00:00:00Z".to_string(),
    };
    build_persona_event(&record)
        .unwrap()
        .sign_with_keys(&nostr::Keys::generate())
        .unwrap()
}

/// Retain a row at the coordinate, optionally pending. Returns the signed
/// event so a caller can stamp a baseline naming it.
fn retain_row(conn: &Connection, kind: u32, d_tag: &str, pending: bool) -> nostr::Event {
    retain_prompt(conn, kind, d_tag, pending, "prompt")
}

/// The same, with control over the persona's system prompt — which is what
/// [`CanonicalProjection`] reads, so it is how a test makes the queued row's
/// content differ from the baseline's.
fn retain_prompt(
    conn: &Connection,
    kind: u32,
    d_tag: &str,
    pending: bool,
    system_prompt: &str,
) -> nostr::Event {
    let event = signed_persona(system_prompt);
    retain_event(
        conn,
        &RetainedEvent {
            kind,
            pubkey: OWNER.to_string(),
            d_tag: d_tag.to_string(),
            content: event.content.to_string(),
            created_at: event.created_at.as_secs() as i64,
            raw_event: {
                use nostr::JsonUtil;
                event.as_json()
            },
            pending_sync: pending,
            event_id: Some(event.id.to_hex()),
        },
    )
    .unwrap();
    event
}

/// Re-queue an already-retained row, the way an edit or a reconcile would.
fn set_pending(conn: &Connection, kind: u32, d_tag: &str) {
    conn.execute(
        "UPDATE persona_events SET pending_sync = 1
         WHERE kind = ?1 AND pubkey = ?2 AND d_tag = ?3",
        rusqlite::params![kind, OWNER, d_tag],
    )
    .unwrap();
}

// ── local_state: reading the observation off a real store ──────────────────

#[test]
fn test_no_local_row_reports_nothing_queued_and_no_baseline() {
    let conn = test_db();
    let observation = local_state(
        &conn,
        OWNER,
        &coordinate(),
        Some(projection("v1")),
        HeadState::Present(head("v1", 100)),
        TombstoneEvidence::NotFound,
    )
    .unwrap();

    assert!(observation.queued.is_none());
    assert!(observation.queued_deletion_at.is_none());
    assert!(observation.baseline.is_none());
}

/// The queued projection is read from the row's own `raw_event`, because those
/// are the bytes the flush loop publishes.
#[test]
fn test_pending_live_row_reports_its_own_queued_projection() {
    let conn = test_db();
    retain_prompt(&conn, KIND_PERSONA, "test-persona", true, "queued-prompt");

    let observation = local_state(
        &conn,
        OWNER,
        &coordinate(),
        Some(projection("whatever-is-on-disk")),
        HeadState::Present(head("v2", 200)),
        TombstoneEvidence::NotFound,
    )
    .unwrap();

    let queued = observation.queued.expect("the pending row must be queued");
    assert!(
        queued.content.contains("queued-prompt"),
        "the queued projection must come from the row's raw_event, got {queued:?}"
    );
    assert!(
        observation.queued_deletion_at.is_none(),
        "a queued edit is not a queued deletion"
    );
}

/// A queued DELETION lives at a different primary key than the record itself.
/// Reporting only the live row was the cross-coordinate hole from review.
#[test]
fn test_pending_tombstone_row_is_reported_for_its_target_coordinate() {
    let conn = test_db();
    // The live row is NOT pending; only the paired tombstone is.
    retain_row(&conn, KIND_PERSONA, "test-persona", false);
    let tombstone = retain_row(&conn, KIND_DELETION, &tombstone_key(), true);

    let observation = local_state(
        &conn,
        OWNER,
        &coordinate(),
        Some(projection("v1")),
        HeadState::Present(head("v2", 200)),
        TombstoneEvidence::NotFound,
    )
    .unwrap();

    assert_eq!(
        observation.queued_deletion_at,
        Some(tombstone.created_at.as_secs() as i64),
        "a pending tombstone must be reported against its TARGET coordinate"
    );
    assert!(
        observation.queued.is_none(),
        "the live row is settled; only the tombstone is queued"
    );
}

/// A settled tombstone row must not pin the coordinate as queued forever.
#[test]
fn test_non_pending_tombstone_row_is_not_reported_as_queued() {
    let conn = test_db();
    retain_row(&conn, KIND_PERSONA, "test-persona", false);
    retain_row(&conn, KIND_DELETION, &tombstone_key(), false);

    let observation = local_state(
        &conn,
        OWNER,
        &coordinate(),
        Some(projection("v1")),
        HeadState::Present(head("v1", 100)),
        TombstoneEvidence::NotFound,
    )
    .unwrap();

    assert!(observation.queued.is_none());
    assert!(observation.queued_deletion_at.is_none());
}

#[test]
fn test_stamped_baseline_is_read_back() {
    let conn = test_db();
    let event = retain_row(&conn, KIND_PERSONA, "test-persona", false);
    set_baseline(
        &conn,
        KIND_PERSONA,
        OWNER,
        "test-persona",
        &event.id.to_hex(),
        "baseline-content",
    )
    .unwrap();

    let observation = local_state(
        &conn,
        OWNER,
        &coordinate(),
        Some(projection("baseline-content")),
        HeadState::Present(head("newer", 200)),
        TombstoneEvidence::NotFound,
    )
    .unwrap();

    assert_eq!(
        observation.baseline.as_ref().map(|b| b.content.as_str()),
        Some("baseline-content")
    );
    // Disk still equals the baseline, so the differing head wins — the exact
    // stale-store case the fix exists for.
    assert_eq!(decide(&observation).decision, Decision::ApplyHead);
}

/// A half-written baseline (id without content, or vice versa) is not a usable
/// provenance record and must read as "no baseline".
#[test]
fn test_partial_baseline_reads_as_absent() {
    let conn = test_db();
    retain_row(&conn, KIND_PERSONA, "test-persona", false);
    conn.execute(
        "UPDATE persona_events SET baseline_event_id = 'someid', baseline_content = NULL
         WHERE kind = ?1 AND pubkey = ?2 AND d_tag = ?3",
        rusqlite::params![KIND_PERSONA, OWNER, "test-persona"],
    )
    .unwrap();

    let observation = local_state(
        &conn,
        OWNER,
        &coordinate(),
        Some(projection("local")),
        HeadState::Present(head("remote", 200)),
        TombstoneEvidence::NotFound,
    )
    .unwrap();

    assert!(observation.baseline.is_none());
    assert_eq!(decide(&observation).decision, Decision::Defer);
}

#[test]
fn test_plan_pairs_every_coordinate_with_its_decision() {
    let base = Observation {
        disk: Some(projection("v1")),
        queued: None,
        head: HeadState::Present(head("v2", 200)),
        baseline: Some(projection("v1")),
        queued_deletion_at: None,
        tombstone: TombstoneEvidence::NotFound,
    };
    let states = vec![
        CoordinateState {
            coordinate: Coordinate {
                kind: KIND_PERSONA,
                d_tag: "adopts".to_string(),
            },
            observation: base.clone(),
        },
        CoordinateState {
            coordinate: Coordinate {
                kind: KIND_PERSONA,
                d_tag: "parks".to_string(),
            },
            observation: Observation {
                disk: Some(projection("edited")),
                head: HeadState::Absent,
                ..base
            },
        },
    ];

    let plan = plan(&states);

    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].0.d_tag, "adopts");
    assert_eq!(plan[0].1.decision, Decision::ApplyHead);
    assert_eq!(plan[1].0.d_tag, "parks");
    assert_eq!(
        plan[1].1.decision,
        Decision::Park(ParkReason::LocalEditWithNoHead)
    );
}

// ── The publication gates (V3.9) ───────────────────────────────────────────

fn resolution(decision: Decision) -> Resolution {
    Resolution {
        decision,
        clear_queued_publish: false,
    }
}

/// The live-row gate is deny-by-default over the decision set, so a variant
/// added later is withheld until someone deliberately admits it.
#[test]
fn test_exactly_the_publishing_decisions_release_the_live_gate() {
    for decision in [
        Decision::SuppressPublish,
        Decision::Park(ParkReason::LocalEditWithNoHead),
        Decision::Park(ParkReason::NoBaselineWithHead),
        Decision::StampBaseline,
        Decision::DeleteLocal,
        Decision::Defer,
        // The deletion publishes from the tombstone row; the live row it
        // deletes must not race it.
        Decision::PublishDeletion,
        // The one adopt-the-relay cell reachable with NO baseline, so it must
        // not leave an open gate behind for a row queued after the pass.
        Decision::RestoreFromRelay,
    ] {
        assert!(
            blocks_publication(&resolution(decision.clone())),
            "{decision:?} must withhold the live row"
        );
    }

    for decision in [
        Decision::PublishLocalEdit,
        // Only reachable with a baseline present and nothing queued, so
        // gating it would strand a later edit and suppress nothing.
        Decision::ApplyHead,
    ] {
        assert!(
            !blocks_publication(&resolution(decision.clone())),
            "{decision:?} must not withhold the live row"
        );
    }
}

/// Exactly one decision releases the tombstone row. Anything else — including
/// the timestamp arbitration falling through to the live cells — means the
/// queued deletion did not win.
#[test]
fn test_only_publish_deletion_releases_the_tombstone_gate() {
    for decision in [
        Decision::PublishLocalEdit,
        Decision::ApplyHead,
        Decision::RestoreFromRelay,
        Decision::StampBaseline,
        Decision::SuppressPublish,
        Decision::DeleteLocal,
        Decision::Defer,
        Decision::Park(ParkReason::NoBaselineWithHead),
        Decision::Park(ParkReason::LocalEditWithNoHead),
    ] {
        assert!(
            blocks_tombstone_publication(&resolution(decision.clone())),
            "{decision:?} must withhold the queued tombstone"
        );
    }
    assert!(!blocks_tombstone_publication(&resolution(
        Decision::PublishDeletion
    )));
}

/// The gate's whole purpose: a suppressed coordinate must actually vanish from
/// the flush loop's work list.
#[test]
fn test_suppressed_coordinate_is_withheld_from_the_flush_loop() {
    let conn = test_db();
    retain_row(&conn, KIND_PERSONA, "test-persona", true);

    assert_eq!(
        get_pending_sync(&conn).unwrap().len(),
        1,
        "precondition: the pending row is publishable"
    );

    let observation = Observation {
        disk: Some(projection("v1")),
        queued: None,
        head: HeadState::Absent,
        baseline: Some(projection("v1")),
        queued_deletion_at: None,
        tombstone: TombstoneEvidence::NotFound,
    };
    apply_gate(
        &conn,
        OWNER,
        &coordinate(),
        &observation,
        &resolution(Decision::SuppressPublish),
    )
    .unwrap();

    assert!(
        get_pending_sync(&conn).unwrap().is_empty(),
        "a suppressed coordinate must not reach the publisher"
    );
    // The content and its pending flag survive — suppression withholds, it
    // does not discard what this device believes.
    let row = get_retained_event(&conn, KIND_PERSONA, OWNER, "test-persona")
        .unwrap()
        .unwrap();
    assert!(row.pending_sync);
}

/// Suppression must be reversible: once the head is visible again the same
/// coordinate resumes publishing, or the gate latches shut permanently.
#[test]
fn test_gate_reopens_when_a_later_decision_publishes() {
    let conn = test_db();
    retain_row(&conn, KIND_PERSONA, "test-persona", true);

    let suppressed = Observation {
        disk: Some(projection("v1")),
        queued: None,
        head: HeadState::Absent,
        baseline: Some(projection("v1")),
        queued_deletion_at: None,
        tombstone: TombstoneEvidence::NotFound,
    };
    apply_gate(
        &conn,
        OWNER,
        &coordinate(),
        &suppressed,
        &resolution(Decision::SuppressPublish),
    )
    .unwrap();
    assert!(is_publish_blocked(&conn, KIND_PERSONA, OWNER, "test-persona").unwrap());

    // Next boot: the head is visible and disk carries a real edit.
    let recovered = Observation {
        disk: Some(projection("edited")),
        head: HeadState::Present(head("v1", 100)),
        ..suppressed
    };
    apply_gate(
        &conn,
        OWNER,
        &coordinate(),
        &recovered,
        &resolution(Decision::PublishLocalEdit),
    )
    .unwrap();

    assert!(!is_publish_blocked(&conn, KIND_PERSONA, OWNER, "test-persona").unwrap());
    assert_eq!(
        get_pending_sync(&conn).unwrap().len(),
        1,
        "the coordinate must be publishable again"
    );
}

/// The gate is stored per coordinate: withholding one persona must not
/// withhold another, or one unreachable head would silence the whole store.
#[test]
fn test_gate_is_scoped_to_its_own_coordinate() {
    let conn = test_db();
    retain_row(&conn, KIND_PERSONA, "test-persona", true);
    retain_row(&conn, KIND_PERSONA, "other-persona", true);

    let observation = Observation {
        disk: Some(projection("v1")),
        queued: None,
        head: HeadState::Absent,
        baseline: Some(projection("v1")),
        queued_deletion_at: None,
        tombstone: TombstoneEvidence::NotFound,
    };
    apply_gate(
        &conn,
        OWNER,
        &coordinate(),
        &observation,
        &resolution(Decision::SuppressPublish),
    )
    .unwrap();

    let pending = get_pending_sync(&conn).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].d_tag, "other-persona");
}

/// The gate has two halves because the rows have two primary keys. A pass that
/// wrote only the live half would compute a deletion suppression and enforce
/// it on the wrong row, leaving the tombstone free to publish.
#[test]
fn test_the_gate_enforces_on_the_tombstone_row_too() {
    let conn = test_db();
    retain_row(&conn, KIND_DELETION, &tombstone_key(), true);

    assert_eq!(
        get_pending_sync(&conn).unwrap().len(),
        1,
        "precondition: the queued tombstone is publishable"
    );

    let observation = Observation {
        disk: None,
        queued: None,
        head: HeadState::Present(head("still-there", 200)),
        baseline: None,
        queued_deletion_at: Some(100),
        tombstone: TombstoneEvidence::NotFound,
    };
    apply_gate(
        &conn,
        OWNER,
        &coordinate(),
        &observation,
        &resolution(Decision::Park(ParkReason::NoBaselineWithHead)),
    )
    .unwrap();

    assert!(
        get_pending_sync(&conn).unwrap().is_empty(),
        "a withheld deletion must not reach the publisher"
    );
    assert!(is_publish_blocked(&conn, KIND_DELETION, OWNER, &tombstone_key()).unwrap());
}

// ── Composition: the whole pass, not one layer ─────────────────────────────
//
// Every test above exercises one layer in isolation, which is exactly how a
// gate that can never fire passed a full suite: `decide` was tested with
// hand-built observations and `apply_gate` with hand-picked decisions, so
// nothing asserted that the decision a REAL pending row produces is one the
// gate actually blocks. These run the composition — retained row →
// `local_state` → `decide` → `apply_gate` → `get_pending_sync` — because that
// path is the product behaviour, and the seam between layers is where the bug
// lived.

/// Build the observation for a coordinate the way the barrier does, from rows
/// that are actually in the store.
fn observe(conn: &Connection, head: HeadState, disk: Option<CanonicalProjection>) -> Observation {
    local_state(
        conn,
        OWNER,
        &coordinate(),
        disk,
        head,
        TombstoneEvidence::NotFound,
    )
    .unwrap()
}

/// Run the real pass over one coordinate and report what the flush loop would
/// then publish.
fn run_pass(conn: &Connection, observation: &Observation) -> (Decision, usize) {
    let plan = run_decision_pass(
        conn,
        OWNER,
        &[CoordinateState {
            coordinate: coordinate(),
            observation: observation.clone(),
        }],
    )
    .unwrap();
    (
        plan[0].1.decision.clone(),
        get_pending_sync(conn).unwrap().len(),
    )
}

/// **Will's bug, end to end, and the regression test for this whole PR.**
///
/// A second install holds the OLD system prompt on disk with a FRESH retention
/// database — so no baseline, which is what a dev build against the same relay
/// always looks like. The good edit was made on the other install and is the
/// relay head. Boot's reconcile has already re-signed the stale disk content
/// at `monotonic_created_at = max(now, head + 1)` and queued it, so it would
/// win the relay's last-write-wins compare on `created_at` every time.
///
/// Composed through the real path rather than asserted on `decide` alone: 43
/// layer-isolated tests passed while the composition was a no-op, and that is
/// the gap class this closes. Zero rows publishable is the only acceptable
/// outcome.
#[test]
fn test_stale_store_with_no_baseline_cannot_revert_a_newer_head() {
    let conn = test_db();
    // The reconcile's queued re-sign of the stale disk content. No baseline is
    // stamped: this install has never agreed to anything at this coordinate.
    retain_prompt(&conn, KIND_PERSONA, "test-persona", true, "old-prompt");
    assert_eq!(
        get_pending_sync(&conn).unwrap().len(),
        1,
        "precondition: without the barrier this row publishes"
    );
    assert!(
        get_baseline(&conn, KIND_PERSONA, OWNER, "test-persona")
            .unwrap()
            .is_none(),
        "precondition: a fresh retention database has no baseline"
    );

    let observation = observe(
        &conn,
        // The good edit, on the relay, at an OLDER wall-clock time than the
        // re-sign — which is precisely why `created_at` cannot arbitrate this.
        HeadState::Present(head("good-edit", 1)),
        Some(projection("old-prompt")),
    );
    let (decision, publishable) = run_pass(&conn, &observation);

    assert_eq!(
        decision,
        Decision::Park(ParkReason::NoBaselineWithHead),
        "the no-baseline-with-head cell must park"
    );
    assert_eq!(
        publishable, 0,
        "the stale revert reached the publisher (decision: {decision:?})"
    );
    // Withheld, not destroyed — the device still holds its copy, and the row
    // keeps its pending flag so a later resolution can act on it.
    let row = get_retained_event(&conn, KIND_PERSONA, OWNER, "test-persona")
        .unwrap()
        .expect("the record must survive suppression");
    assert!(row.pending_sync);
}

/// The same bug in its second shape: a queued DELETION from a no-baseline
/// store against a live head. Worse than the revert, because an a-tag delete
/// destroys content this install has never seen rather than reverting it.
#[test]
fn test_stale_store_with_no_baseline_cannot_delete_a_live_head() {
    let conn = test_db();
    retain_row(&conn, KIND_DELETION, &tombstone_key(), true);

    let observation = observe(&conn, HeadState::Present(head("still-there", 1)), None);
    let (decision, publishable) = run_pass(&conn, &observation);

    assert_eq!(decision, Decision::Park(ParkReason::NoBaselineWithHead));
    assert_eq!(
        publishable, 0,
        "an unprovable deletion reached the publisher (decision: {decision:?})"
    );
}

/// **The tombstone escape, composed over BOTH rows.**
///
/// A coordinate's record and its queued tombstone occupy different primary
/// keys — `(kind, owner, d_tag)` versus `(5, owner, "<kind>:<d_tag>")` — and
/// the flush loop reads them independently. A pass that computes the right
/// decision but enforces it on one key is indistinguishable from no gate at
/// all for the other, so the no-baseline invariant is only meaningful stated
/// over both.
///
/// The deletion here is deliberately NEWER than the head, which is the exact
/// input `deletion_at >= head.created_at` would resolve to `PublishDeletion`.
/// It must not get there: the no-baseline gate runs first, so an install that
/// cannot show it ever agreed to anything at this coordinate never destroys a
/// head it has not seen. Asserted through `get_pending_sync` — what the flush
/// loop actually selects — not through the decision alone.
#[test]
fn test_no_baseline_store_withholds_both_the_record_and_its_tombstone() {
    let conn = test_db();
    // Both halves queued at once: the reconcile's re-sign of stale disk
    // content, and a tombstone targeting the same coordinate.
    retain_prompt(&conn, KIND_PERSONA, "test-persona", true, "old-prompt");
    let tombstone = retain_row(&conn, KIND_DELETION, &tombstone_key(), true);
    assert_eq!(
        get_pending_sync(&conn).unwrap().len(),
        2,
        "precondition: without the barrier both rows publish"
    );
    assert!(
        get_baseline(&conn, KIND_PERSONA, OWNER, "test-persona")
            .unwrap()
            .is_none(),
        "precondition: a fresh retention database has no baseline"
    );

    // The head predates the tombstone, so cell 4's timestamp arbitration would
    // hand this to `PublishDeletion` if the baseline gate did not run first.
    let head_created_at = tombstone.created_at.as_secs() as i64 - 1;
    let observation = observe(
        &conn,
        HeadState::Present(head("good-edit", head_created_at)),
        Some(projection("old-prompt")),
    );
    assert!(
        observation.queued_deletion_at.unwrap() >= head_created_at,
        "precondition: the queued deletion must be the newer of the two"
    );

    let (decision, publishable) = run_pass(&conn, &observation);

    assert_eq!(
        decision,
        Decision::Park(ParkReason::NoBaselineWithHead),
        "the no-baseline gate must take this before the timestamp compare"
    );
    assert_eq!(
        publishable, 0,
        "a row escaped the flush-selection gate (decision: {decision:?})"
    );
    assert!(
        is_publish_blocked(&conn, KIND_PERSONA, OWNER, "test-persona").unwrap(),
        "the record's own row must be gated"
    );
    assert!(
        is_publish_blocked(&conn, KIND_DELETION, OWNER, &tombstone_key()).unwrap(),
        "the tombstone row must be gated at its own primary key"
    );
}

/// The mirror case, and the reason suppression cannot simply block everything
/// pending: a genuine local edit against a head this install has already
/// agreed on must still publish, or the fix freezes every agent config.
#[test]
fn test_genuine_local_edit_still_publishes() {
    let conn = test_db();
    let event = retain_prompt(&conn, KIND_PERSONA, "test-persona", true, "my-new-edit");
    // The baseline is what this install last agreed was published; the queued
    // row has since moved past it.
    set_baseline(
        &conn,
        KIND_PERSONA,
        OWNER,
        "test-persona",
        &event.id.to_hex(),
        "published-v1",
    )
    .unwrap();

    let observation = observe(
        &conn,
        HeadState::Present(head("published-v1", 100)),
        Some(projection("my-new-edit")),
    );
    let (decision, publishable) = run_pass(&conn, &observation);

    assert_eq!(
        decision,
        Decision::PublishLocalEdit,
        "a post-baseline edit must be recognized as an edit"
    );
    assert_eq!(publishable, 1, "the user's edit must still reach the relay");
}

/// A vacuous re-sign — the queued row publishes exactly what the baseline
/// already says — is DROPPED, not gated. Gating would hold it forever, since
/// it can never become publishable.
#[test]
fn test_vacuous_re_sign_is_cleared_rather_than_held() {
    let conn = test_db();
    let event = retain_prompt(&conn, KIND_PERSONA, "test-persona", true, "agreed");
    // Baseline names this exact event, so the queued projection equals it.
    set_baseline(
        &conn,
        KIND_PERSONA,
        OWNER,
        "test-persona",
        &event.id.to_hex(),
        &event.content,
    )
    .unwrap();

    let observation = observe(
        &conn,
        HeadState::Present(head("newer-elsewhere", 200)),
        Some(projection(&event.content)),
    );
    let (_, publishable) = run_pass(&conn, &observation);

    assert_eq!(publishable, 0);
    let row = get_retained_event(&conn, KIND_PERSONA, OWNER, "test-persona")
        .unwrap()
        .unwrap();
    assert!(
        !row.pending_sync,
        "a re-sign carrying no new intent must be dropped, not held pending forever"
    );
}

/// A pending row whose disk content already equals the head is the
/// crash-convergence case: stamp the baseline and stop, rather than burn a
/// `created_at` bump republishing identical content.
#[test]
fn test_pending_row_matching_the_head_stamps_and_stops() {
    let conn = test_db();
    retain_row(&conn, KIND_PERSONA, "test-persona", true);

    let observation = observe(
        &conn,
        HeadState::Present(head("agreed", 100)),
        Some(projection("agreed")),
    );
    let (decision, publishable) = run_pass(&conn, &observation);

    assert_eq!(decision, Decision::Park(ParkReason::NoBaselineWithHead));
    assert_eq!(publishable, 0, "identical content must not be republished");
}

/// `StampBaseline` has to actually write the baseline. It is the input every
/// other cell depends on, so a decision computed and dropped leaves the table
/// permanently in its no-provenance state.
#[test]
fn test_stamp_baseline_records_the_head_as_provenance() {
    let conn = test_db();
    retain_row(&conn, KIND_PERSONA, "test-persona", false);

    let observation = observe(
        &conn,
        HeadState::Present(head("agreed", 100)),
        Some(projection("agreed")),
    );
    run_pass(&conn, &observation);

    assert_eq!(
        get_baseline(&conn, KIND_PERSONA, OWNER, "test-persona").unwrap(),
        Some(("id-agreed".to_string(), "agreed".to_string())),
        "the baseline must be stamped from the head, content and id together"
    );
}

/// The two-boot sequence that makes the fix durable rather than a one-shot:
/// boot 1 converges and stamps the baseline, boot 2 uses that provenance to
/// recognize the stale copy the reconcile re-queued.
#[test]
fn test_stamped_baseline_lets_the_next_boot_suppress_a_revert() {
    let conn = test_db();
    let event = retain_prompt(&conn, KIND_PERSONA, "test-persona", false, "v1");
    // The head IS the event this store already holds — the converged state
    // boot 1 is supposed to recognize.
    let converged = Head {
        event_id: event.id.to_hex(),
        created_at: event.created_at.as_secs() as i64,
        projection: CanonicalProjection::from_event(&event),
    };
    let disk = converged.projection.clone();

    // Boot 1: disk equals the head, so the baseline is stamped from it.
    let settled = observe(
        &conn,
        HeadState::Present(converged.clone()),
        Some(disk.clone()),
    );
    let (decision, _) = run_pass(&conn, &settled);
    assert_eq!(decision, Decision::StampBaseline);
    assert_eq!(
        get_baseline(&conn, KIND_PERSONA, OWNER, "test-persona").unwrap(),
        Some((converged.event_id.clone(), converged.projection.content)),
        "boot 1 must establish provenance"
    );

    // Boot 2: the other install published a newer edit; the reconcile on THIS
    // install re-queued its unchanged copy at a manufactured timestamp.
    set_pending(&conn, KIND_PERSONA, "test-persona");
    let stale = observe(
        &conn,
        HeadState::Present(head("good-edit", 1)),
        Some(disk.clone()),
    );
    let (decision, publishable) = run_pass(&conn, &stale);

    assert_eq!(
        publishable, 0,
        "with a baseline, the stale copy must be recognized and withheld \
         (decision: {decision:?})"
    );
    // Recognized as vacuous rather than merely gated: the queued row says
    // exactly what the baseline already says, so it is dropped and the newer
    // head is what this coordinate should adopt.
    assert_eq!(decision, Decision::ApplyHead);
    assert!(
        !get_retained_event(&conn, KIND_PERSONA, OWNER, "test-persona")
            .unwrap()
            .unwrap()
            .pending_sync
    );
}

/// A queued deletion from a store that CAN prove its provenance still reaches
/// the relay. Suppressing every tombstone would resurrect records the user
/// deliberately removed.
#[test]
fn test_a_provable_queued_tombstone_still_publishes() {
    let conn = test_db();
    let tombstone = retain_row(&conn, KIND_DELETION, &tombstone_key(), true);
    // Provenance at the target coordinate: this install agreed to what is
    // published there, so its deletion is arbitrable (cell 4).
    retain_prompt(&conn, KIND_PERSONA, "test-persona", false, "agreed");
    let event = get_retained_event(&conn, KIND_PERSONA, OWNER, "test-persona")
        .unwrap()
        .unwrap();
    set_baseline(
        &conn,
        KIND_PERSONA,
        OWNER,
        "test-persona",
        event.event_id.as_deref().unwrap(),
        &event.content,
    )
    .unwrap();

    // The head is older than the tombstone, so the deletion wins.
    let observation = observe(
        &conn,
        HeadState::Present(head("still-there", 1)),
        Some(projection(&event.content)),
    );
    assert_eq!(
        observation.queued_deletion_at,
        Some(tombstone.created_at.as_secs() as i64)
    );
    let (decision, publishable) = run_pass(&conn, &observation);

    assert_eq!(decision, Decision::PublishDeletion);
    assert_eq!(publishable, 1, "the queued deletion must reach the relay");
    assert_eq!(get_pending_sync(&conn).unwrap()[0].kind, KIND_DELETION);
}
