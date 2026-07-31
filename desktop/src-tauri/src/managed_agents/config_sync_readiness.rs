//! Per-scope readiness latch for the boot config barrier.
//!
//! Tracks the single-flight state of the `Unready → InProgress → Ready`
//! transition for the active retention scope, preventing a concurrent inline
//! flush barrier from certifying readiness before the reconcile+migration that
//! follows `spawn_event_sync_with_held_claim` has finished queuing its rows.
//!
//! # State machine
//!
//! ```text
//!   apply_workspace (force)  ─────────────────────────►  InProgress (from any state)
//!   flush retry (claim, only when Unready)  ───────────►  InProgress
//!   boot barrier success  ─────────────────────────────►  Ready(db_path)
//!   boot barrier error / abandon  ─────────────────────►  Unready (if still current gen)
//!   flush: InProgress → skip tick; Ready(p) → publish
//! ```
//!
//! Two claim variants:
//!
//! - [`claim_in_progress`] — CAS from `Unready` only. Returns `None` when
//!   the state is already `InProgress` or `Ready`. Used by the flush retry
//!   path.
//!
//! - [`force_claim_in_progress`] — preempting claim; transitions from **any**
//!   state (including `InProgress` or `Ready`) to `InProgress` **and bumps
//!   the generation counter**. Used by `apply_workspace`, which invalidates
//!   whatever the prior scope certified.
//!
//! Both variants return a [`ReadinessClaim`] RAII guard that records the
//! generation it was issued under. Latch mutations (`complete` and `Drop`)
//! only take effect if the latch generation still matches the claim's. A
//! preempted claim is a structural no-op — it cannot certify or clear
//! readiness after `force_claim_in_progress` has invalidated it.
//!
//! # Ownership identity
//!
//! The generation counter is the ownership identity. When `force_claim_in_progress`
//! bumps the generation, every outstanding claim from the previous generation
//! becomes permanently inert: its `complete` call is a no-op, and its `Drop`
//! does not reset the latch. This closes both legs of the preempted-claim race:
//!
//! - Leg 1 (stale barrier certifies Ready): stale claim's `complete()` sees
//!   a generation mismatch → no-op → latch stays `InProgress`.
//! - Leg 2 (stale drop destroys InProgress): stale claim's `Drop` sees a
//!   generation mismatch → no-op → force-claim's `InProgress` is preserved.

use std::path::PathBuf;

#[cfg(test)]
use std::path::Path;

/// The current readiness state for the active scope's publication gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadinessState {
    /// No barrier has been run for the current scope.
    Unready,
    /// A reconcile + barrier is in progress. The flush loop must skip this tick.
    InProgress,
    /// The barrier completed successfully for the named scope.
    Ready(PathBuf),
}

// ── Internal latch state ──────────────────────────────────────────────────────

struct LatchState {
    state: ReadinessState,
    /// Monotonically-increasing generation. Incremented by every
    /// `force_claim_in_progress` call. A claim records the generation it was
    /// issued under; transitions are only applied when the current generation
    /// matches the claim's, structurally invalidating all prior claims.
    generation: u64,
}

impl LatchState {
    const fn new() -> Self {
        Self {
            state: ReadinessState::Unready,
            generation: 0,
        }
    }
}

// ── Process-global latch ──────────────────────────────────────────────────────

#[cfg(not(test))]
static READINESS: std::sync::Mutex<LatchState> = std::sync::Mutex::new(LatchState::new());

// In tests, each test thread gets an isolated latch so tests cannot interfere.
#[cfg(test)]
thread_local! {
    static READINESS: std::sync::Mutex<LatchState> =
        const { std::sync::Mutex::new(LatchState::new()) };
}

// ── Public interface ──────────────────────────────────────────────────────────

/// RAII guard that holds the `InProgress` claim.
///
/// Owns a generation stamp matching the latch generation at the time the claim
/// was issued. All latch mutations go through the claim:
///
/// - [`ReadinessClaim::complete`] — transitions to `Ready(db_path)` if and
///   only if the latch generation still matches. Preempted claims are no-ops.
/// - `Drop` — resets to `Unready` if and only if the latch generation still
///   matches. A panic or early return in the reconcile+barrier window leaves
///   the scope fail-closed and retryable, but only if this claim is still
///   current. A preempted claim's drop is a no-op.
///
/// Created by [`claim_in_progress`] or [`force_claim_in_progress`].
#[must_use = "dropping a ReadinessClaim without calling complete() resets latch to Unready if still current generation"]
pub struct ReadinessClaim {
    generation: u64,
}

impl ReadinessClaim {
    fn new(generation: u64) -> Self {
        Self { generation }
    }

    /// Return `true` iff this claim's generation still matches the latch.
    ///
    /// Used by the barrier's enforcement phase: while holding
    /// `managed_agents_store_lock`, call this immediately before
    /// [`run_decision_pass`][super::config_sync::run_decision_pass] to ensure
    /// a preempted barrier cannot overwrite gate decisions produced by the
    /// current owner's barrier.
    pub fn is_current(&self) -> bool {
        with_lock(|latch| latch.generation == self.generation).unwrap_or(false)
    }

    /// Transition the latch to `Ready(db_path)` if and only if this claim is
    /// still current (latch generation matches). A preempted claim's call is
    /// a silent no-op — the preemptor owns the latch now.
    ///
    /// Consumes the claim so that `Drop` does not also attempt a reset.
    pub fn complete(self, db_path: PathBuf) {
        let _ = with_lock(|latch| {
            if latch.generation == self.generation {
                latch.state = ReadinessState::Ready(db_path);
            }
            // Mismatch → preempted claim, no-op.
        });
        // Prevent Drop from resetting to Unready: we consumed self, so Drop
        // will not run (the value is moved into this function's scope and the
        // mem::forget below prevents it). We use mem::forget here because there
        // is no other way to prevent Drop after consuming self.
        std::mem::forget(self);
    }
}

impl Drop for ReadinessClaim {
    fn drop(&mut self) {
        // Panic or early return: reset to Unready only if this claim is still
        // current. A preempted claim (generation mismatch) is a no-op.
        let _ = with_lock(|latch| {
            if latch.generation == self.generation {
                latch.state = ReadinessState::Unready;
            }
        });
    }
}

/// Attempt a CAS from `Unready` → `InProgress`.
///
/// Returns `Some(ReadinessClaim)` if the caller won the transition and now
/// owns the reconcile+barrier run. Returns `None` if the state was already
/// `InProgress` or `Ready` — the caller must not start a barrier.
///
/// The returned guard's `Drop` resets to `Unready` only if the claim is
/// still current (generation matches). Call [`ReadinessClaim::complete`] on
/// success.
pub fn claim_in_progress() -> Option<ReadinessClaim> {
    with_lock(|latch| {
        if latch.state == ReadinessState::Unready {
            latch.state = ReadinessState::InProgress;
            Some(ReadinessClaim::new(latch.generation))
        } else {
            None
        }
    })
    .unwrap_or(None)
}

/// Preempting claim: transitions from **any** state (including `InProgress`
/// or `Ready`) to `InProgress` and **bumps the generation counter**.
///
/// Used by `apply_workspace`, which invalidates whatever the prior scope
/// certified. Bumping the generation structurally invalidates all outstanding
/// claims from the previous generation: their `complete` and `Drop` become
/// no-ops, so a stale flush barrier cannot certify readiness or reset
/// `InProgress` mid-migration.
///
/// Returns a `ReadinessClaim` RAII guard stamped with the new generation
/// (always succeeds).
pub fn force_claim_in_progress() -> ReadinessClaim {
    let gen = with_lock(|latch| {
        latch.generation = latch.generation.wrapping_add(1);
        latch.state = ReadinessState::InProgress;
        latch.generation
    })
    .unwrap_or(0);
    ReadinessClaim::new(gen)
}

/// Reset to `Unready` unconditionally.
///
/// Use for test cleanup and exceptional reset paths only. Normal transitions
/// go through [`ReadinessClaim::complete`] or the `Drop` impl.
#[cfg(test)]
pub fn mark_unready() {
    let _ = with_lock(|latch| {
        latch.state = ReadinessState::Unready;
    });
}

/// `true` iff the scope is `Ready` for exactly `db_path`.
#[cfg(test)]
pub fn is_ready_for(db_path: &Path) -> bool {
    with_lock(|latch| matches!(&latch.state, ReadinessState::Ready(p) if p == db_path))
        .unwrap_or(false)
}

/// `true` iff a reconcile+barrier is currently in-flight.
#[cfg(test)]
pub fn is_in_progress() -> bool {
    with_lock(|latch| latch.state == ReadinessState::InProgress).unwrap_or(false)
}

/// Read a snapshot of the current state.
pub fn readiness_state() -> Option<ReadinessState> {
    with_lock(|latch| latch.state.clone()).ok()
}

/// Read the current generation counter (for tests only).
#[cfg(test)]
pub fn current_generation() -> u64 {
    with_lock(|latch| latch.generation).unwrap_or(0)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

#[cfg(not(test))]
fn with_lock<R>(f: impl FnOnce(&mut LatchState) -> R) -> Result<R, String> {
    READINESS
        .lock()
        .map(|mut g| f(&mut g))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
fn with_lock<R>(f: impl FnOnce(&mut LatchState) -> R) -> Result<R, String> {
    READINESS.with(|m| m.lock().map(|mut g| f(&mut g)).map_err(|e| e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_is_unready() {
        mark_unready(); // reset for test isolation
        assert!(!is_in_progress());
        assert_eq!(readiness_state(), Some(ReadinessState::Unready));
    }

    #[test]
    fn test_claim_in_progress_from_unready_succeeds() {
        mark_unready();
        let path = PathBuf::from("/scope/db");
        let claim = claim_in_progress();
        assert!(claim.is_some(), "first claim must succeed");
        assert!(is_in_progress());
        claim.unwrap().complete(path);
        mark_unready(); // cleanup
    }

    #[test]
    fn test_claim_in_progress_is_single_flight() {
        mark_unready();
        let path = PathBuf::from("/scope/db");
        let claim = claim_in_progress();
        assert!(claim.is_some(), "first claim wins");
        assert!(
            claim_in_progress().is_none(),
            "second claim rejected while InProgress"
        );
        claim.unwrap().complete(path);
        mark_unready(); // cleanup
    }

    #[test]
    fn test_claim_rejected_when_ready() {
        mark_unready();
        let path = PathBuf::from("/scope/db");
        let claim = claim_in_progress().unwrap();
        claim.complete(path.clone());
        assert!(claim_in_progress().is_none(), "cannot claim when Ready");
        assert!(is_ready_for(&path));
        mark_unready(); // cleanup
    }

    #[test]
    fn test_complete_sets_ready_for_path() {
        mark_unready();
        let path = PathBuf::from("/scope/db");
        let claim = claim_in_progress().unwrap();
        claim.complete(path.clone());
        assert!(is_ready_for(&path));
        assert!(!is_ready_for(Path::new("/other/db")));
        mark_unready(); // cleanup
    }

    #[test]
    fn test_mark_unready_resets_to_claimable() {
        mark_unready();
        let path = PathBuf::from("/scope/db");
        let claim = claim_in_progress().unwrap();
        claim.complete(path);
        mark_unready();
        assert_eq!(readiness_state(), Some(ReadinessState::Unready));
        let claim2 = claim_in_progress();
        assert!(claim2.is_some(), "after reset, claim must succeed");
        drop(claim2); // drop without complete → resets to Unready
    }

    #[test]
    fn test_workspace_switch_cycle() {
        mark_unready();
        let path = PathBuf::from("/scope/db");
        let claim = claim_in_progress().unwrap();
        claim.complete(path.clone());
        assert!(is_ready_for(&path));
        mark_unready();
        assert!(!is_ready_for(&path));
        let claim2 = claim_in_progress().unwrap();
        claim2.complete(path.clone());
        assert!(is_ready_for(&path));
        mark_unready(); // cleanup
    }

    #[test]
    fn test_claim_drop_without_complete_resets_to_unready() {
        mark_unready();
        {
            let _claim = claim_in_progress().unwrap();
            assert!(is_in_progress());
            // drop without complete
        }
        assert_eq!(
            readiness_state(),
            Some(ReadinessState::Unready),
            "dropped claim without complete must reset to Unready"
        );
    }

    #[test]
    fn test_force_claim_preempts_ready() {
        mark_unready();
        let path = PathBuf::from("/scope/db");
        let first = claim_in_progress().unwrap();
        first.complete(path.clone());
        assert!(is_ready_for(&path));
        // force_claim must preempt Ready
        let fc = force_claim_in_progress();
        assert!(
            is_in_progress(),
            "force_claim must set InProgress even from Ready"
        );
        drop(fc); // cleanup via RAII
    }

    #[test]
    fn test_force_claim_preempts_in_progress() {
        mark_unready();
        let first = claim_in_progress().unwrap();
        assert!(is_in_progress());
        let gen_before = current_generation();
        // force_claim must preempt even an existing InProgress
        let second = force_claim_in_progress();
        assert!(is_in_progress());
        let gen_after = current_generation();
        assert_eq!(
            gen_after,
            gen_before + 1,
            "force_claim must bump generation"
        );
        // first is now stale — its drop must be a no-op (generation mismatch)
        drop(first);
        assert!(
            is_in_progress(),
            "stale drop must not reset latch to Unready"
        );
        drop(second); // cleanup via RAII
    }

    // ── (g) stale-claim completion is a no-op ────────────────────────────────
    //
    // Setup: flush wins claim → force_claim preempts → stale claim calls
    // complete() → latch stays InProgress; flush claim_in_progress still
    // rejected (current owner is the force-claim generation).
    #[test]
    fn test_stale_claim_complete_is_noop() {
        mark_unready();
        let path = PathBuf::from("/scope/db");

        // Flush wins the initial claim.
        let stale_claim = claim_in_progress().unwrap();
        let stale_gen = current_generation();

        // apply_workspace force-claims, bumping the generation.
        let force_claim = force_claim_in_progress();
        let new_gen = current_generation();
        assert_eq!(new_gen, stale_gen + 1, "generation must increment");
        assert!(is_in_progress());

        // Stale barrier finishes and tries to certify Ready — must be a no-op.
        stale_claim.complete(path.clone());
        assert!(
            is_in_progress(),
            "stale complete must not transition latch to Ready"
        );
        assert!(
            !is_ready_for(&path),
            "stale complete must not set Ready(path)"
        );

        // Force-claim's InProgress must still block a new flush claim.
        assert!(
            claim_in_progress().is_none(),
            "flush claim must still be rejected after stale complete"
        );

        // Cleanup: force_claim still owns the latch.
        drop(force_claim);
        assert_eq!(readiness_state(), Some(ReadinessState::Unready));
    }

    // ── (h) stale-claim drop is a no-op ──────────────────────────────────────
    //
    // Setup: flush wins claim → force_claim preempts → stale claim is dropped
    // unresolved → latch stays InProgress (force-claim still owns it).
    #[test]
    fn test_stale_claim_drop_is_noop() {
        mark_unready();

        // Flush wins the initial claim.
        let stale_claim = claim_in_progress().unwrap();
        let stale_gen = current_generation();

        // apply_workspace force-claims.
        let force_claim = force_claim_in_progress();
        let new_gen = current_generation();
        assert_eq!(new_gen, stale_gen + 1);
        assert!(is_in_progress());

        // Stale barrier errors out — its RAII drop must be a no-op.
        drop(stale_claim);
        assert!(
            is_in_progress(),
            "stale drop must not reset force-claim's InProgress to Unready"
        );

        // Cleanup.
        drop(force_claim);
        assert_eq!(readiness_state(), Some(ReadinessState::Unready));
    }
}
