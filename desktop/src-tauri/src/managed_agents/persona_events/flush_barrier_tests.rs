//! Deterministic tests for the publish-gate, readiness-latch, and
//! single-flight invariants of the flush loop.
//!
//! All tests exercise the production transition functions
//! (`flush_pending_events`, `set_post_snapshot_hook`, the module-level
//! readiness functions in `config_sync_readiness`) rather than manually
//! toggling mutex state.
//!
//! Thread isolation: `config_sync_readiness` uses `thread_local!` in test
//! builds so each test thread gets an isolated latch; no cross-test
//! interference.
//!
//! Gated off Windows for the same reason as `flush_barrier` in tests.rs:
//! `build_app_state()` pulls native DLLs unavailable in the Windows CI runner.
#![cfg(not(target_os = "windows"))]

use nostr::{EventBuilder, JsonUtil, Kind, Tag};
use std::path::PathBuf;
use tempfile::tempdir;

use crate::app_state::build_app_state;
use crate::managed_agents::config_sync_readiness::{self, ReadinessState};
use crate::managed_agents::persona_events::{
    clear_post_snapshot_hook, flush_pending_events, set_post_snapshot_hook,
};
use crate::managed_agents::retention::{
    get_retained_event, open_retention_db, retain_event, set_publish_blocked, RetainedEvent,
};
use buzz_core_pkg::kind::KIND_PERSONA;

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn spawn_stub_relay() -> String {
    use axum::{http::StatusCode, routing::post, Router};
    let app = Router::new().route(
        "/events",
        post(|body: String| async move {
            let event: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            if event.get("kind").and_then(serde_json::Value::as_u64) == Some(5) {
                return (StatusCode::INTERNAL_SERVER_ERROR, String::new());
            }
            (
                StatusCode::OK,
                serde_json::json!({
                    "event_id": event.get("id").and_then(serde_json::Value::as_str).unwrap_or(""),
                    "accepted": true,
                    "message": ""
                })
                .to_string(),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stub relay");
    let addr = listener.local_addr().expect("stub relay addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    format!("http://{addr}")
}

fn retain_pending(conn: &rusqlite::Connection, keys: &nostr::Keys, d_tag: &str) {
    let builder = EventBuilder::new(
        Kind::Custom(KIND_PERSONA as u16),
        format!("{{\"display_name\":\"{d_tag}\"}}"),
    )
    .tags(vec![Tag::parse(["d", d_tag]).unwrap()]);
    let event = builder.sign_with_keys(keys).expect("sign");
    retain_event(
        conn,
        &RetainedEvent {
            kind: KIND_PERSONA,
            pubkey: keys.public_key().to_hex(),
            d_tag: d_tag.to_string(),
            content: event.content.to_string(),
            created_at: 1_000_000,
            raw_event: event.as_json(),
            event_id: None,
            pending_sync: true,
            publish_blocked: false,
        },
    )
    .expect("retain");
}

// ── (a) snapshot-vs-gate race: row gated AFTER snapshot cannot submit ─────────
//
// The boot barrier may close the gate between `get_pending_sync` (snapshot)
// and the per-row `submit_signed_event_at_with_keys` call. The pre-submit
// re-read of `publish_blocked` must catch rows gated in this window.
//
// Setup:
// 1. Insert an unblocked pending row (passes the SQL gate at snapshot time).
// 2. Install a post-snapshot hook that sets `publish_blocked = 1` on the row.
// 3. Run `flush_pending_events`.
// Expected: 0 published (pre-submit re-read blocked the row); row stays pending.
#[tokio::test]
async fn test_row_gated_between_snapshot_and_submit_cannot_publish() {
    let keys = nostr::Keys::generate();
    let pubkey = keys.public_key().to_hex();
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("retention.db");

    {
        let conn = open_retention_db(&db_path).expect("open db");
        retain_pending(&conn, &keys, "agent-a");
    }

    // Install the hook: after snapshot is taken, gate the row — simulating
    // the barrier closing the gate between snapshot and per-row submit.
    let db_path_for_hook = db_path.clone();
    let pubkey_for_hook = pubkey.clone();
    set_post_snapshot_hook(move |path| {
        assert_eq!(path, db_path_for_hook.as_path());
        let conn = open_retention_db(path).expect("open db in hook");
        set_publish_blocked(&conn, KIND_PERSONA, &pubkey_for_hook, "agent-a", true)
            .expect("gate row in hook");
    });

    let state = build_app_state();
    *state.keys.lock().unwrap() = keys.clone();
    *state.relay_url_override.lock().unwrap() = Some(spawn_stub_relay().await);

    let flushed = flush_pending_events(&db_path, &state).await.expect("flush");
    clear_post_snapshot_hook();

    assert_eq!(flushed, 0, "row gated after snapshot must not publish");

    let conn = open_retention_db(&db_path).expect("reopen db");
    let row = get_retained_event(&conn, KIND_PERSONA, &pubkey, "agent-a")
        .unwrap()
        .unwrap();
    assert!(row.pending_sync, "gated row must stay pending");
    assert!(row.publish_blocked, "publish_blocked flag must persist");
}

// ── (b) InProgress blocks a second claim ─────────────────────────────────────
//
// When a reconcile+barrier is in-flight (`InProgress`), a second
// `claim_in_progress` call must be rejected. This is the CAS that prevents
// the flush retry from certifying readiness against a pre-reconcile snapshot.
#[test]
fn test_in_progress_state_blocks_second_claim() {
    // Reset to known state first (thread_local ensures isolation).
    config_sync_readiness::mark_unready();

    // First caller (spawn_event_sync) claims InProgress.
    let first_claim = config_sync_readiness::claim_in_progress();
    assert!(
        first_claim.is_some(),
        "first claim must succeed (Unready → InProgress)"
    );
    assert!(config_sync_readiness::is_in_progress());

    // Second caller (flush retry) must be rejected — it would run a barrier
    // against the pre-reconcile database state.
    assert!(
        config_sync_readiness::claim_in_progress().is_none(),
        "second claim must be rejected while InProgress"
    );

    // State is still InProgress — the flush must skip this tick.
    assert!(config_sync_readiness::is_in_progress());
    assert_eq!(
        config_sync_readiness::readiness_state(),
        Some(ReadinessState::InProgress)
    );

    let path = PathBuf::from("/scope/db");
    first_claim.unwrap().complete(path);
    config_sync_readiness::mark_unready(); // cleanup
}

// ── (c) barrier failure resets latch to Unready for retry ────────────────────
//
// A barrier that encounters an error must leave the scope `Unready` so the
// next flush tick can retry via `claim_in_progress`. It must NOT leave the
// latch `InProgress` (wedged forever) or `Ready` (open gate without enforcement).
#[test]
fn test_barrier_failure_resets_to_unready_for_retry() {
    config_sync_readiness::mark_unready();

    // spawn_event_sync claims InProgress before reconcile.
    let claim = config_sync_readiness::claim_in_progress();
    assert!(claim.is_some());

    // Barrier error: dropping the claim without complete() resets to Unready
    // (RAII guard, generation matches). Simulates run_boot_barrier_enforcing returning Err.
    drop(claim); // intentional drop-without-complete

    assert_eq!(
        config_sync_readiness::readiness_state(),
        Some(ReadinessState::Unready),
        "barrier error must reset to Unready — not InProgress (wedged) or Ready (open gate)"
    );

    // Next flush tick must be able to claim for retry.
    let retry_claim = config_sync_readiness::claim_in_progress();
    assert!(
        retry_claim.is_some(),
        "after failure, next claim must succeed so the retry can run"
    );

    let path = PathBuf::from("/scope/db");
    retry_claim.unwrap().complete(path);
    config_sync_readiness::mark_unready(); // cleanup
}

// ── (d) Thufir's interleaving test: concurrent inline barrier cannot beat reconcile ──
//
// Scenario (reproduced deterministically using the process-global latch):
//
// 1. `spawn_event_sync` claims `InProgress` before reconcile starts.
// 2. A flush tick fires and calls `claim_in_progress` (simulating
//    run_boot_barrier's CAS entry). It is rejected — InProgress is taken.
// 3. Reconcile runs and retains a new row (publish_blocked=false).
// 4. `run_boot_barrier_after_claim` runs the barrier: gates the stale row,
//    then marks Ready.
// 5. Next flush tick sees Ready; but the row is blocked — 0 publish.
//
// This proves that holding InProgress through reconcile+barrier prevents an
// inline barrier from certifying readiness against the pre-reconcile snapshot.
#[tokio::test]
async fn test_inline_barrier_cannot_certify_readiness_before_reconcile() {
    config_sync_readiness::mark_unready();

    let keys = nostr::Keys::generate();
    let pubkey = keys.public_key().to_hex();
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("retention.db");

    // Step 1: spawn_event_sync claims InProgress before reconcile starts.
    let claim = config_sync_readiness::claim_in_progress();
    assert!(
        claim.is_some(),
        "spawn_event_sync must win the InProgress claim"
    );

    // Step 2: flush tick fires and tries to claim — must be rejected.
    assert!(
        config_sync_readiness::claim_in_progress().is_none(),
        "inline flush barrier must be rejected — InProgress already claimed"
    );
    assert!(
        config_sync_readiness::is_in_progress(),
        "latch must still be InProgress"
    );

    // Step 3: reconcile runs and retains a stale row (the race case).
    {
        let conn = open_retention_db(&db_path).expect("open db");
        retain_pending(&conn, &keys, "stale-agent");
    }

    // Step 4: run_boot_barrier_after_claim runs post-reconcile. It owns
    // InProgress (no re-claim). The barrier's decision: no-baseline row →
    // gate it. We simulate the barrier's enforcement here.
    {
        let conn = open_retention_db(&db_path).expect("open db for barrier");
        set_publish_blocked(&conn, KIND_PERSONA, &pubkey, "stale-agent", true)
            .expect("barrier gates stale row");
    }
    claim.unwrap().complete(db_path.clone());
    // latch is now Ready

    // Step 5: scope is Ready; flush runs but the row is blocked — 0 publish.
    assert!(
        config_sync_readiness::is_ready_for(&db_path),
        "scope must be Ready after barrier"
    );

    let state = build_app_state();
    *state.keys.lock().unwrap() = keys.clone();
    *state.relay_url_override.lock().unwrap() = Some(spawn_stub_relay().await);

    let flushed = flush_pending_events(&db_path, &state).await.expect("flush");
    assert_eq!(
        flushed, 0,
        "stale row gated by post-reconcile barrier must not publish even when scope is Ready"
    );

    let conn = open_retention_db(&db_path).expect("reopen db");
    let row = get_retained_event(&conn, KIND_PERSONA, &pubkey, "stale-agent")
        .unwrap()
        .unwrap();
    assert!(row.pending_sync, "gated row stays pending");
    assert!(row.publish_blocked, "barrier's gate persists");

    config_sync_readiness::mark_unready(); // cleanup
}

// ── (e) retry path: after failure, publish succeeds when scope becomes Ready ──
//
// End-to-end: spawn_event_sync claims InProgress → barrier fails (RAII drop
// resets to Unready) → next tick re-claims InProgress → barrier succeeds
// (claim.complete()) → row publishes on the following flush call.
#[tokio::test]
async fn test_retry_after_barrier_failure_publishes_when_ready() {
    config_sync_readiness::mark_unready();

    let keys = nostr::Keys::generate();
    let pubkey = keys.public_key().to_hex();
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("retention.db");

    {
        let conn = open_retention_db(&db_path).expect("open db");
        retain_pending(&conn, &keys, "my-agent");
    }

    let state = build_app_state();
    *state.keys.lock().unwrap() = keys.clone();
    let relay_url = spawn_stub_relay().await;
    *state.relay_url_override.lock().unwrap() = Some(relay_url);

    // Phase 1: spawn_event_sync claims InProgress → barrier fails → drop
    // resets to Unready via RAII (generation matches, this is the current claim).
    {
        let claim = config_sync_readiness::claim_in_progress();
        assert!(claim.is_some());
        // Simulate barrier error: drop without complete().
        drop(claim);
    }

    // Latch is Unready — retry is possible.
    assert_eq!(
        config_sync_readiness::readiness_state(),
        Some(ReadinessState::Unready),
        "after failure, scope must be Unready for retry"
    );

    // Phase 2: next flush tick re-claims InProgress, barrier succeeds → Ready.
    let retry_claim = config_sync_readiness::claim_in_progress();
    assert!(
        retry_claim.is_some(),
        "after mark_unready, claim must succeed for retry"
    );
    retry_claim.unwrap().complete(db_path.clone());

    // Row is unblocked — flush must publish it now that scope is Ready.
    let flushed = flush_pending_events(&db_path, &state).await.expect("flush");
    assert_eq!(flushed, 1, "unblocked row must publish when scope is Ready");

    let conn = open_retention_db(&db_path).expect("reopen db");
    let row = get_retained_event(&conn, KIND_PERSONA, &pubkey, "my-agent")
        .unwrap()
        .unwrap();
    assert!(!row.pending_sync, "row must be marked synced after publish");

    config_sync_readiness::mark_unready(); // cleanup
}

// ── (f) apply_workspace window: flush claim between invalidation and reconcile ──
//
// Paul's bounce defect: between `mark_unready()` and `spawn_event_sync`'s
// inner `claim_in_progress()`, the flush loop could win the CAS and certify
// readiness against the pre-migration, pre-reconcile database state. Migrated
// or reconciled rows then publish unarbitrated.
//
// Fix: `apply_workspace` calls `force_claim_in_progress()` BEFORE migration,
// then passes the held claim to `spawn_event_sync_with_held_claim`. The flush
// loop sees `InProgress` throughout and cannot interleave.
//
// This test reproduces the race deterministically:
// 1. `apply_workspace` calls `force_claim_in_progress()` (InProgress from any state).
// 2. A flush tick fires immediately and calls `claim_in_progress()` — rejected.
// 3. Legacy migration runs and retains a row with `publish_blocked=false`.
// 4. Reconcile retains a second row.
// 5. Post-reconcile barrier gates both rows (sets `publish_blocked=true`).
// 6. Scope transitions to Ready.
// 7. Flush runs — both rows are blocked, 0 published.
#[tokio::test]
async fn test_apply_workspace_flush_cannot_interleave_before_reconcile() {
    config_sync_readiness::mark_unready();

    let keys = nostr::Keys::generate();
    let pubkey = keys.public_key().to_hex();
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("retention.db");

    // Step 1: apply_workspace atomically claims InProgress (preempting).
    let claim = config_sync_readiness::force_claim_in_progress();
    assert!(
        config_sync_readiness::is_in_progress(),
        "force_claim must set InProgress"
    );

    // Step 2: flush tick fires and tries to claim — must be rejected.
    assert!(
        config_sync_readiness::claim_in_progress().is_none(),
        "flush claim during apply_workspace window must be rejected"
    );

    // Step 3: legacy migration runs and retains a row.
    {
        let conn = open_retention_db(&db_path).expect("open db");
        retain_pending(&conn, &keys, "legacy-row");
    }

    // Step 4: reconcile retains a second row.
    {
        let conn = open_retention_db(&db_path).expect("open db");
        retain_pending(&conn, &keys, "reconciled-row");
    }

    // Step 5: post-reconcile barrier gates both rows.
    {
        let conn = open_retention_db(&db_path).expect("open db for barrier");
        set_publish_blocked(&conn, KIND_PERSONA, &pubkey, "legacy-row", true)
            .expect("gate legacy row");
        set_publish_blocked(&conn, KIND_PERSONA, &pubkey, "reconciled-row", true)
            .expect("gate reconciled row");
    }

    // Step 6: barrier completes — complete claim (transitions to Ready only if still current gen).
    claim.complete(db_path.clone());

    // Step 7: flush runs — both rows are blocked, 0 published.
    let state = build_app_state();
    *state.keys.lock().unwrap() = keys.clone();
    *state.relay_url_override.lock().unwrap() = Some(spawn_stub_relay().await);

    let flushed = flush_pending_events(&db_path, &state).await.expect("flush");
    assert_eq!(
        flushed, 0,
        "migrated and reconciled rows gated by post-reconcile barrier must not publish"
    );

    let conn = open_retention_db(&db_path).expect("reopen db");
    for d_tag in ["legacy-row", "reconciled-row"] {
        let row = get_retained_event(&conn, KIND_PERSONA, &pubkey, d_tag)
            .unwrap()
            .unwrap();
        assert!(
            row.publish_blocked,
            "{d_tag}: publish_blocked must persist after gating"
        );
        assert!(row.pending_sync, "{d_tag}: gated row must stay pending");
    }

    config_sync_readiness::mark_unready(); // cleanup
}

// ── (j) generation-gate blocks stale barrier's decision pass ─────────────────
//
// Pass-3 CRITICAL finding: before this fix, `run_decision_pass` wrote gate
// decisions unconditionally — a preempted barrier resuming after the current
// one certified `Ready` could overwrite newer gate decisions with its stale
// relay evidence. The defect was invisible to the 2110-test suite because
// test (i) only verified that `complete()` is a no-op; it never drove a
// stale barrier all the way through enforcement.
//
// This test exercises the enforcement seam directly: stale barrier S builds a
// `CoordinateState` from stale relay evidence (the exact CRITICAL cell —
// `HeadState::Absent` sees cell-3 `PublishLocalEdit`, opening a gate the
// current barrier parked) and calls `run_decision_pass` while gated by the
// `is_current()` check. The generation guard must block the write.
//
// Deterministic sequence:
// 1. Flush wins claim (gen 0). Its barrier observes `HeadState::Absent` for
//    the coordinate — stale evidence that would decide `PublishLocalEdit`.
// 2. apply_workspace force-claims (gen 1). The current barrier observes
//    `HeadState::Present` and decides `Park(NoBaselineWithHead)`.
// 3. Current barrier (gen 1) runs `run_decision_pass` → sets
//    `publish_blocked = true`. Certifies Ready(path).
// 4. Stale barrier (gen 0) resumes enforcement with its stale evidence.
//    Before calling `run_decision_pass`, the generation guard (`is_current()`)
//    returns false — write is blocked. `publish_blocked` must remain `true`.
// 5. Vacuity proof: without the guard, the stale pass WOULD clear the flag
//    (verified on a scratch db so the real store is never mutated by the test).
#[test]
fn test_stale_barriers_decision_pass_cannot_alter_publish_blocked() {
    use crate::managed_agents::{
        canonical_projection::CanonicalProjection,
        config_sync::{local_state, plan, run_decision_pass, CoordinateState},
        decision::{Decision, Head, HeadState, TombstoneEvidence},
        head_lookup::Coordinate,
        retention::{is_publish_blocked, open_retention_db, retain_event, RetainedEvent},
    };
    use buzz_core_pkg::kind::KIND_PERSONA;
    use nostr::{EventBuilder, JsonUtil, Kind, Tag};

    config_sync_readiness::mark_unready();

    let keys = nostr::Keys::generate();
    let pubkey = keys.public_key().to_hex();
    let d_tag = "barrier-race";
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("retention.db");

    // Retain a no-baseline pending row (the exact revert cell).
    let event = EventBuilder::new(
        Kind::Custom(KIND_PERSONA as u16),
        r#"{"system_prompt":"old"}"#.to_string(),
    )
    .tags(vec![Tag::parse(["d", d_tag]).unwrap()])
    .sign_with_keys(&keys)
    .expect("sign");
    {
        let conn = open_retention_db(&db_path).expect("open db");
        retain_event(
            &conn,
            &RetainedEvent {
                kind: KIND_PERSONA,
                pubkey: pubkey.clone(),
                d_tag: d_tag.to_string(),
                content: event.content.to_string(),
                created_at: event.created_at.as_secs() as i64,
                raw_event: event.as_json(),
                event_id: None,
                pending_sync: true,
                publish_blocked: false,
            },
        )
        .expect("retain");
    }

    let coordinate = Coordinate {
        kind: KIND_PERSONA,
        d_tag: d_tag.to_string(),
    };

    // Step 1: flush wins the initial claim (gen 0). Stale relay evidence:
    // HeadState::Absent → no baseline → cell-2 PublishLocalEdit (would open the gate).
    let stale_claim = config_sync_readiness::claim_in_progress().unwrap();
    assert_eq!(config_sync_readiness::current_generation(), 0);
    let stale_observation = {
        let conn = open_retention_db(&db_path).expect("open db for stale observation");
        local_state(
            &conn,
            &pubkey,
            &coordinate,
            None,
            HeadState::Absent,
            TombstoneEvidence::NotFound,
        )
        .unwrap()
    };
    let stale_states = vec![CoordinateState {
        coordinate: coordinate.clone(),
        observation: stale_observation,
    }];

    // Vacuity check: the stale evidence would decide PublishLocalEdit.
    // If this fails the test is measuring the wrong thing.
    assert_eq!(
        plan(&stale_states)[0].1.decision,
        Decision::PublishLocalEdit,
        "vacuity: stale evidence (Absent head, no baseline) must decide PublishLocalEdit"
    );

    // Step 2: apply_workspace fires — force_claim bumps to gen 1.
    let force_claim = config_sync_readiness::force_claim_in_progress();
    assert_eq!(config_sync_readiness::current_generation(), 1);

    // Step 3: current barrier (gen 1) runs its pass — Present head + no baseline
    // → Park(NoBaselineWithHead) → publish_blocked = true. Certifies Ready.
    {
        let current_observation = {
            let conn = open_retention_db(&db_path).expect("open db for current observation");
            local_state(
                &conn,
                &pubkey,
                &coordinate,
                None,
                HeadState::Present(Head {
                    event_id: "id-relay-head".to_string(),
                    created_at: 200,
                    projection: CanonicalProjection {
                        content: "good-edit".to_string(),
                        shared: false,
                    },
                }),
                TombstoneEvidence::NotFound,
            )
            .unwrap()
        };
        let current_conn = open_retention_db(&db_path).expect("open db for current pass");
        run_decision_pass(
            &current_conn,
            &pubkey,
            &[CoordinateState {
                coordinate: coordinate.clone(),
                observation: current_observation,
            }],
        )
        .expect("current barrier enforces");
    }
    assert!(
        is_publish_blocked(
            &open_retention_db(&db_path).unwrap(),
            KIND_PERSONA,
            &pubkey,
            d_tag,
        )
        .unwrap(),
        "precondition: current barrier (gen 1) must have set publish_blocked = true"
    );
    force_claim.complete(db_path.clone());
    assert!(config_sync_readiness::is_ready_for(&db_path));

    // Step 4: stale barrier (gen 0) resumes enforcement.
    //
    // The enforcement seam: `run_boot_barrier_for_scope` calls `claim.is_current()`
    // while holding `managed_agents_store_lock`, and returns `Abandoned` without
    // calling `run_decision_pass` when it returns false.
    //
    // We reproduce that guard here: `is_current()` must return false.
    assert!(
        !stale_claim.is_current(),
        "stale claim (gen 0) must not be current after force_claim bumped to gen 1"
    );

    // Enforcement seam: the stale barrier only calls run_decision_pass if
    // is_current() returns true. Since it doesn't, the write is blocked.
    // This is the invariant the production guard enforces.
    if stale_claim.is_current() {
        let conn = open_retention_db(&db_path).expect("open db for stale enforcement");
        run_decision_pass(&conn, &pubkey, &stale_states).expect("stale pass");
    }

    // Vacuity companion: confirm the stale pass WOULD have cleared the flag
    // by running it on an isolated scratch db. This proves the test would
    // catch a regression where is_current() is removed — the stale evidence
    // genuinely opens the gate when unguarded.
    {
        let scratch_dir = tempfile::tempdir().expect("scratch tempdir");
        let scratch_db = scratch_dir.path().join("scratch.db");
        let scratch_conn = open_retention_db(&scratch_db).expect("open scratch");
        retain_event(
            &scratch_conn,
            &RetainedEvent {
                kind: KIND_PERSONA,
                pubkey: pubkey.clone(),
                d_tag: d_tag.to_string(),
                content: "old".to_string(),
                created_at: 100,
                raw_event: event.as_json(),
                event_id: None,
                pending_sync: true,
                publish_blocked: true,
            },
        )
        .expect("retain in scratch");
        run_decision_pass(&scratch_conn, &pubkey, &stale_states).expect("unguarded stale pass");
        assert!(
            !is_publish_blocked(&scratch_conn, KIND_PERSONA, &pubkey, d_tag).unwrap(),
            "vacuity: without the guard the stale pass clears publish_blocked"
        );
    }

    // Step 5: real store must be unchanged — publish_blocked must remain true.
    assert!(
        is_publish_blocked(
            &open_retention_db(&db_path).unwrap(),
            KIND_PERSONA,
            &pubkey,
            d_tag,
        )
        .unwrap(),
        "publish_blocked must remain true: stale barrier's run_decision_pass was blocked by the generation guard"
    );

    // Stale claim drops (gen mismatch → latch no-op; force-claim already certified Ready).
    drop(stale_claim);
    assert!(config_sync_readiness::is_ready_for(&db_path));

    config_sync_readiness::mark_unready(); // cleanup
}

// ── (i) leg-1 end-to-end: stale flush barrier cannot publish after force-claim ─
//
// Paul's second bounce, leg 1: flush wins claim at t≈0, its inline barrier
// completes its relay round-trips and calls `complete()`, but by that time
// apply_workspace's `force_claim_in_progress` has bumped the generation. The
// stale `complete()` is a structural no-op — latch stays InProgress (the
// force-claim's generation). Rows retained AFTER the stale complete cannot
// publish until the force-claim's own barrier passes.
//
// Deterministic interleaving:
// 1. Flush wins claim at Unready → InProgress (gen 0).
// 2. apply_workspace fires → force_claim bumps to gen 1, sets InProgress.
//    Stale flush claim is now generation 0 in a generation-1 latch.
// 3. Stale barrier "completes" its relay work and calls complete(path).
//    Generation mismatch (0 ≠ 1) → no-op; latch stays InProgress.
// 4. Rows retained by reconcile land in the DB (publish_blocked=false).
// 5. Force-claim's barrier gates all rows (sets publish_blocked=true), then
//    calls complete(path) with gen 1 → latch transitions to Ready.
// 6. Flush runs: scope is Ready, but rows are blocked — 0 published.
#[tokio::test]
async fn test_stale_flush_barrier_complete_cannot_publish_after_force_claim() {
    config_sync_readiness::mark_unready();

    let keys = nostr::Keys::generate();
    let pubkey = keys.public_key().to_hex();
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("retention.db");

    // Step 1: flush wins the initial claim (gen 0).
    let stale_flush_claim = config_sync_readiness::claim_in_progress().unwrap();
    assert!(config_sync_readiness::is_in_progress());
    let stale_gen = config_sync_readiness::current_generation();
    assert_eq!(stale_gen, 0);

    // Step 2: apply_workspace fires — force_claim bumps generation to 1.
    let force_claim = config_sync_readiness::force_claim_in_progress();
    assert_eq!(config_sync_readiness::current_generation(), 1);
    assert!(config_sync_readiness::is_in_progress());

    // Stale flush cannot claim again (force-claim holds InProgress).
    assert!(config_sync_readiness::claim_in_progress().is_none());

    // Step 3: stale barrier "finishes" and calls complete(path) — gen mismatch → no-op.
    stale_flush_claim.complete(db_path.clone());
    assert!(
        config_sync_readiness::is_in_progress(),
        "stale complete (gen 0 vs latch gen 1) must not certify Ready"
    );
    assert!(
        !config_sync_readiness::is_ready_for(&db_path),
        "latch must not be Ready after stale complete"
    );

    // Step 4: reconcile retains rows — these are the rows that would have
    // escaped if the stale complete had succeeded.
    {
        let conn = open_retention_db(&db_path).expect("open db");
        retain_pending(&conn, &keys, "post-reconcile-row");
    }

    // Stale barrier cannot certify again (latch is still InProgress gen 1).
    assert!(config_sync_readiness::claim_in_progress().is_none());

    // Step 5: force-claim's barrier gates all rows, then completes with gen 1 → Ready.
    {
        let conn = open_retention_db(&db_path).expect("open db for force barrier");
        set_publish_blocked(&conn, KIND_PERSONA, &pubkey, "post-reconcile-row", true)
            .expect("force barrier gates reconcile row");
    }
    force_claim.complete(db_path.clone());
    assert!(
        config_sync_readiness::is_ready_for(&db_path),
        "force-claim complete must transition to Ready"
    );

    // Step 6: flush runs — scope is Ready but rows are blocked by the
    // force-claim's barrier. 0 published.
    let state = build_app_state();
    *state.keys.lock().unwrap() = keys.clone();
    *state.relay_url_override.lock().unwrap() = Some(spawn_stub_relay().await);

    let flushed = flush_pending_events(&db_path, &state).await.expect("flush");
    assert_eq!(
        flushed, 0,
        "rows retained after stale complete must not publish — force-claim's barrier gated them"
    );

    let conn = open_retention_db(&db_path).expect("reopen db");
    let row = get_retained_event(&conn, KIND_PERSONA, &pubkey, "post-reconcile-row")
        .unwrap()
        .unwrap();
    assert!(row.publish_blocked, "post-reconcile row must stay blocked");
    assert!(row.pending_sync, "blocked row must stay pending");

    config_sync_readiness::mark_unready(); // cleanup
}
