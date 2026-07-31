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

    first_claim.unwrap().resolve();
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

    // Barrier error: dropping the claim without resolve() resets to Unready
    // (RAII guard). Simulates run_boot_barrier_enforcing returning Err.
    drop(claim); // intentional drop-without-resolve

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

    retry_claim.unwrap().resolve();
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
    claim.unwrap().resolve();
    config_sync_readiness::mark_ready(db_path.clone());

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
// (mark_ready) → row publishes on the following flush call.
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
    // resets to Unready via RAII.
    {
        let claim = config_sync_readiness::claim_in_progress();
        assert!(claim.is_some());
        // Simulate barrier error: drop without resolve().
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
    retry_claim.unwrap().resolve();
    config_sync_readiness::mark_ready(db_path.clone());

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

    // Step 6: barrier completes — resolve claim and mark Ready.
    claim.resolve();
    config_sync_readiness::mark_ready(db_path.clone());

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
