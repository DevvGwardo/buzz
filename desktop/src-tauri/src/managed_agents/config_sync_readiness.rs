//! Per-scope readiness latch for the boot config barrier.
//!
//! Tracks the single-flight state of the `Unready → InProgress → Ready`
//! transition for the active retention scope, preventing a concurrent inline
//! flush barrier from certifying readiness before the reconcile that follows
//! `spawn_event_sync` has finished queuing its rows.
//!
//! # State machine
//!
//! ```text
//!   apply_workspace  ─────────────────────────────────►  Unready
//!   spawn_event_sync (claim)  ─────────────────────────►  InProgress
//!   flush retry (claim, only when Unready)  ───────────►  InProgress
//!   run_boot_barrier success  ─────────────────────────►  Ready(db_path)
//!   run_boot_barrier error / abandon  ─────────────────►  Unready
//!   flush: InProgress → skip tick; Ready(p) → publish
//! ```
//!
//! Exactly one caller at a time owns the `Unready → InProgress` transition.
//! `claim_in_progress` is a compare-and-swap under the process-global lock:
//! it succeeds only from `Unready` and returns `false` when the state is
//! already `InProgress` or `Ready`. An inline flush retry that fires while
//! `spawn_event_sync`'s reconcile is running will see `InProgress` and skip
//! its tick — it cannot certify readiness against the pre-reconcile database.

use std::path::{Path, PathBuf};

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

// ── Process-global latch ──────────────────────────────────────────────────────

#[cfg(not(test))]
static READINESS: std::sync::Mutex<ReadinessState> = std::sync::Mutex::new(ReadinessState::Unready);

// In tests, each test thread gets an isolated latch so tests cannot interfere.
#[cfg(test)]
thread_local! {
    static READINESS: std::sync::Mutex<ReadinessState> =
        const { std::sync::Mutex::new(ReadinessState::Unready) };
}

// ── Public interface ──────────────────────────────────────────────────────────

/// Attempt a CAS from `Unready` → `InProgress`.
///
/// Returns `true` if the caller won the transition and now owns the
/// reconcile+barrier run. Returns `false` if the state was already
/// `InProgress` or `Ready` — the caller must not start a barrier.
pub fn claim_in_progress() -> bool {
    with_lock(|state| {
        if *state == ReadinessState::Unready {
            *state = ReadinessState::InProgress;
            true
        } else {
            false
        }
    })
    .unwrap_or(false)
}

/// Mark the scope `Ready` for the given `db_path`.
pub fn mark_ready(db_path: PathBuf) {
    let _ = with_lock(|state| {
        *state = ReadinessState::Ready(db_path);
    });
}

/// Reset to `Unready`. Called on barrier error, workspace switch, or any
/// path that must leave the scope fail-closed.
pub fn mark_unready() {
    let _ = with_lock(|state| {
        *state = ReadinessState::Unready;
    });
}

/// `true` iff the scope is `Ready` for exactly `db_path`.
pub fn is_ready_for(db_path: &Path) -> bool {
    with_lock(|state| matches!(&*state, ReadinessState::Ready(p) if p == db_path)).unwrap_or(false)
}

/// `true` iff a reconcile+barrier is currently in-flight.
#[cfg(test)]
pub fn is_in_progress() -> bool {
    with_lock(|state| *state == ReadinessState::InProgress).unwrap_or(false)
}

/// Read a snapshot of the current state.
pub fn readiness_state() -> Option<ReadinessState> {
    with_lock(|state| state.clone()).ok()
}

// ── Internal helpers ──────────────────────────────────────────────────────────

#[cfg(not(test))]
fn with_lock<R>(f: impl FnOnce(&mut ReadinessState) -> R) -> Result<R, String> {
    READINESS
        .lock()
        .map(|mut g| f(&mut g))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
fn with_lock<R>(f: impl FnOnce(&mut ReadinessState) -> R) -> Result<R, String> {
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
        assert!(claim_in_progress(), "first claim must succeed");
        assert!(is_in_progress());
        mark_unready(); // cleanup
    }

    #[test]
    fn test_claim_in_progress_is_single_flight() {
        mark_unready();
        assert!(claim_in_progress(), "first claim wins");
        assert!(
            !claim_in_progress(),
            "second claim rejected while InProgress"
        );
        mark_unready(); // cleanup
    }

    #[test]
    fn test_claim_rejected_when_ready() {
        mark_unready();
        let path = PathBuf::from("/scope/db");
        mark_ready(path.clone());
        assert!(!claim_in_progress(), "cannot claim when Ready");
        assert!(is_ready_for(&path));
        mark_unready(); // cleanup
    }

    #[test]
    fn test_mark_ready_then_is_ready_for() {
        mark_unready();
        let path = PathBuf::from("/scope/db");
        claim_in_progress();
        mark_ready(path.clone());
        assert!(is_ready_for(&path));
        assert!(!is_ready_for(Path::new("/other/db")));
        mark_unready(); // cleanup
    }

    #[test]
    fn test_mark_unready_resets_to_claimable() {
        mark_unready();
        claim_in_progress();
        mark_unready();
        assert_eq!(readiness_state(), Some(ReadinessState::Unready));
        assert!(claim_in_progress(), "after reset, claim must succeed");
        mark_unready(); // cleanup
    }

    #[test]
    fn test_workspace_switch_cycle() {
        mark_unready();
        let path = PathBuf::from("/scope/db");
        claim_in_progress();
        mark_ready(path.clone());
        assert!(is_ready_for(&path));
        mark_unready();
        assert!(!is_ready_for(&path));
        assert!(claim_in_progress());
        mark_ready(path.clone());
        assert!(is_ready_for(&path));
        mark_unready(); // cleanup
    }
}
