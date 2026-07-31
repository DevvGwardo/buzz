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
//!   run_boot_barrier success  ─────────────────────────►  Ready(db_path)
//!   run_boot_barrier error / abandon  ─────────────────►  Unready
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
//!   state (including `InProgress` or `Ready`) to `InProgress`. Used by
//!   `apply_workspace`, which invalidates whatever the prior scope certified.
//!
//! Both variants return a [`ReadinessClaim`] RAII guard. The guard resets the
//! latch to `Unready` on drop unless [`ReadinessClaim::resolve`] has been
//! called first. This ensures a panic inside the reconcile+barrier window
//! always leaves the scope fail-closed and retryable rather than permanently
//! wedged at `InProgress`.

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

/// RAII guard that holds the `InProgress` claim.
///
/// Resets the latch to `Unready` on drop unless [`ReadinessClaim::resolve`]
/// has been called first. This ensures a panic (or early return) inside the
/// reconcile+barrier window always leaves the scope fail-closed and retryable.
///
/// Created by [`claim_in_progress`] or [`force_claim_in_progress`].
#[must_use = "dropping a ReadinessClaim without calling resolve() resets latch to Unready"]
pub struct ReadinessClaim {
    resolved: bool,
}

impl ReadinessClaim {
    fn new() -> Self {
        Self { resolved: false }
    }

    /// Mark the claim as explicitly resolved (either `mark_ready` or
    /// `mark_unready` will handle the final state transition). Call this
    /// before the explicit resolution to prevent the drop guard from
    /// redundantly resetting to `Unready` again.
    pub fn resolve(mut self) {
        self.resolved = true;
        // Drop runs immediately but the flag prevents the reset.
    }
}

impl Drop for ReadinessClaim {
    fn drop(&mut self) {
        if !self.resolved {
            // Panic or early return: leave the scope fail-closed and retryable.
            mark_unready();
        }
    }
}

/// Attempt a CAS from `Unready` → `InProgress`.
///
/// Returns `Some(ReadinessClaim)` if the caller won the transition and now
/// owns the reconcile+barrier run. Returns `None` if the state was already
/// `InProgress` or `Ready` — the caller must not start a barrier.
///
/// The returned guard resets to `Unready` on drop unless
/// [`ReadinessClaim::resolve`] is called first.
pub fn claim_in_progress() -> Option<ReadinessClaim> {
    let won = with_lock(|state| {
        if *state == ReadinessState::Unready {
            *state = ReadinessState::InProgress;
            true
        } else {
            false
        }
    })
    .unwrap_or(false);
    if won {
        Some(ReadinessClaim::new())
    } else {
        None
    }
}

/// Preempting claim: transitions from **any** state (including `InProgress`
/// or `Ready`) to `InProgress`.
///
/// Used by `apply_workspace`, which invalidates whatever the prior scope
/// certified. Because it transitions unconditionally, the flush loop will see
/// `InProgress` immediately after this call and skip its tick — even if the
/// prior scope was `Ready`.
///
/// Returns a `ReadinessClaim` RAII guard (always succeeds).
pub fn force_claim_in_progress() -> ReadinessClaim {
    let _ = with_lock(|state| {
        *state = ReadinessState::InProgress;
    });
    ReadinessClaim::new()
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
        let claim = claim_in_progress();
        assert!(claim.is_some(), "first claim must succeed");
        assert!(is_in_progress());
        claim.unwrap().resolve();
        mark_unready(); // cleanup
    }

    #[test]
    fn test_claim_in_progress_is_single_flight() {
        mark_unready();
        let claim = claim_in_progress();
        assert!(claim.is_some(), "first claim wins");
        assert!(
            claim_in_progress().is_none(),
            "second claim rejected while InProgress"
        );
        claim.unwrap().resolve();
        mark_unready(); // cleanup
    }

    #[test]
    fn test_claim_rejected_when_ready() {
        mark_unready();
        let path = PathBuf::from("/scope/db");
        mark_ready(path.clone());
        assert!(claim_in_progress().is_none(), "cannot claim when Ready");
        assert!(is_ready_for(&path));
        mark_unready(); // cleanup
    }

    #[test]
    fn test_mark_ready_then_is_ready_for() {
        mark_unready();
        let path = PathBuf::from("/scope/db");
        let claim = claim_in_progress().unwrap();
        claim.resolve();
        mark_ready(path.clone());
        assert!(is_ready_for(&path));
        assert!(!is_ready_for(Path::new("/other/db")));
        mark_unready(); // cleanup
    }

    #[test]
    fn test_mark_unready_resets_to_claimable() {
        mark_unready();
        let claim = claim_in_progress().unwrap();
        claim.resolve();
        mark_unready();
        assert_eq!(readiness_state(), Some(ReadinessState::Unready));
        let claim2 = claim_in_progress();
        assert!(claim2.is_some(), "after reset, claim must succeed");
        claim2.unwrap().resolve();
        mark_unready(); // cleanup
    }

    #[test]
    fn test_workspace_switch_cycle() {
        mark_unready();
        let path = PathBuf::from("/scope/db");
        let claim = claim_in_progress().unwrap();
        claim.resolve();
        mark_ready(path.clone());
        assert!(is_ready_for(&path));
        mark_unready();
        assert!(!is_ready_for(&path));
        let claim2 = claim_in_progress().unwrap();
        claim2.resolve();
        mark_ready(path.clone());
        assert!(is_ready_for(&path));
        mark_unready(); // cleanup
    }

    #[test]
    fn test_claim_drop_without_resolve_resets_to_unready() {
        mark_unready();
        {
            let _claim = claim_in_progress().unwrap();
            assert!(is_in_progress());
            // drop without resolve
        }
        assert_eq!(
            readiness_state(),
            Some(ReadinessState::Unready),
            "dropped claim without resolve must reset to Unready"
        );
    }

    #[test]
    fn test_force_claim_preempts_ready() {
        mark_unready();
        let path = PathBuf::from("/scope/db");
        mark_ready(path.clone());
        assert!(is_ready_for(&path));
        // force_claim must preempt Ready
        let claim = force_claim_in_progress();
        assert!(
            is_in_progress(),
            "force_claim must set InProgress even from Ready"
        );
        claim.resolve();
        mark_unready(); // cleanup
    }

    #[test]
    fn test_force_claim_preempts_in_progress() {
        mark_unready();
        let first = claim_in_progress().unwrap();
        assert!(is_in_progress());
        // force_claim must preempt even an existing InProgress
        let second = force_claim_in_progress();
        assert!(is_in_progress());
        // resolve the first claim (already replaced, no-op since resolved=true)
        first.resolve();
        second.resolve();
        mark_unready(); // cleanup
    }
}
