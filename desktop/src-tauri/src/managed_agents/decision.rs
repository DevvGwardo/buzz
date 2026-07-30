//! The boot decision table (V3.2, as amended by V3.10 and V3.11).
//!
//! One pure function, [`decide`], maps the observable state of a single config
//! coordinate onto the action the boot pass should take. It performs no I/O so
//! that every cell is testable directly, and so the ordering between them is
//! visible in one place rather than spread across the caller.
//!
//! # Why a table instead of a content diff
//!
//! Agent configs sync as NIP-33 replaceable events resolved last-write-wins on
//! `created_at`, and the boot reconcile re-signs local disk content at
//! `monotonic_created_at = max(now, head + 1)`. A store that is merely *stale*
//! therefore mints events that are always strictly newer than the edits they
//! revert. Content equality alone cannot tell "the user changed this" from
//! "this store never learned about the newer head", so the pass needs
//! provenance — the baseline — and not just a diff.
//!
//! # Why `created_at` decides almost nothing here (V3.11)
//!
//! V3.2's original row 1 resolved a queued local edit against the relay head by
//! "greater `created_at` wins". Against this failure mode that is not
//! arbitration at all: the queued event's timestamp is manufactured by the very
//! path being stopped, so the stale store wins that comparison every single
//! time. Row 1 as written ratified the laundering vector.
//!
//! V3.11 splits it by evidence class. A queued **edit** is resolved against the
//! baseline, never against a timestamp. A queued **deletion** keeps timestamp
//! arbitration, but only downstream of the baseline gates — and it is sound
//! there precisely because every timestamp that reaches it was minted by an
//! interactive save or by the relay, never by a reconcile re-sign.
//!
//! # The suppression/deletion split (V3.10)
//!
//! Head absence and local deletion are decided on different evidence because
//! the cost of being wrong is wildly asymmetric. Missing head → stop
//! publishing, and nothing else; if that read was wrong the cost is one boot of
//! deferral. Removing a local record requires positive tombstone evidence,
//! because for a managed agent the removal is irreversible: the relay
//! projection is a strict subset of the record, so `private_key_nsec`,
//! `auth_tag`, `env_vars`, and the runtime config cannot be restored from it.
//! The retention scope key is only `owner || relay_url`, so any relay-side
//! history loss under the same URL presents as *every* coordinate being
//! head-absent at once — under a delete-on-absence rule that wipes the device.

use super::canonical_projection::CanonicalProjection;

/// What the boot pass observes about one coordinate, after the pulls have run
/// and before anything is written.
#[derive(Debug, Clone)]
pub struct Observation {
    /// The canonical projection of the record currently on disk, or `None`
    /// when the record is absent from the local store.
    pub disk: Option<CanonicalProjection>,
    /// The projection of the event QUEUED for publish at this coordinate.
    ///
    /// This, not [`Self::disk`], is what the flush loop would put on the relay,
    /// so it is what the baseline arbitration compares. The two agree in both
    /// minting paths today — the reconcile signs disk content, an interactive
    /// save writes both — but the table must not assume that silently: it is
    /// the queued bytes that publish, and a divergence would mean deciding
    /// about content that is not the content going out.
    pub queued: Option<CanonicalProjection>,
    /// The relay head's state. `Absent` and `LookupFailed` are distinct: only a
    /// completed, exact, best-available (replica-lag-exposed) lookup can report absence.
    pub head: HeadState,
    /// The projection this install last agreed was published for this
    /// coordinate. `None` before the baseline is established.
    pub baseline: Option<CanonicalProjection>,
    /// `created_at` of a queued deletion targeting this coordinate, if one is
    /// queued.
    ///
    /// The only timestamp the table consults, and it is honest: a tombstone is
    /// only ever minted by an interactive delete — a real user action at a real
    /// time — never by the reconcile re-sign that manufactures timestamps.
    pub queued_deletion_at: Option<i64>,
    /// Positive evidence that the owner deleted this coordinate: a verified
    /// owner-authored kind:5 naming it, from a scan that ran to completion.
    pub tombstone: TombstoneEvidence,
}

/// A relay head: what it publishes, when it was published, and the event id
/// that published it.
///
/// The id travels with the projection because a baseline is a claim about a
/// specific event on the relay, not about content in the abstract. Stamping a
/// baseline from a projection alone would record "this is what's published"
/// with no way to tell later whether the row still holds that same event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub event_id: String,
    pub created_at: i64,
    pub projection: CanonicalProjection,
}

/// The result of looking up a coordinate's head on the relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadState {
    /// A head exists, with this projection.
    Present(Head),
    /// A completed lookup found no head.
    ///
    /// Only reachable from an exact per-coordinate query
    /// (`{kinds, authors, #d}`, `limit: 2`); never inferred from a combined
    /// inventory, which cannot bound absence due to server-side page caps.
    Absent,
    /// The lookup did not complete. Never treated as absence.
    LookupFailed,
}

/// Whether the owner positively deleted this coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TombstoneEvidence {
    /// A verified owner-authored kind:5 names this exact coordinate.
    Found,
    /// The scan ran to completion and found no tombstone for it.
    NotFound,
    /// The scan did not complete. Never treated as `NotFound`.
    ScanIncomplete,
}

/// What the boot pass should do with one coordinate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Record that disk and head already agree. Stamps or repairs the
    /// baseline; publishes nothing.
    StampBaseline,
    /// Adopt the relay's version onto disk and stamp the baseline from it.
    ApplyHead,
    /// Recreate a locally missing record from the relay head.
    RestoreFromRelay,
    /// A genuine post-baseline local edit: publish the queued event.
    PublishLocalEdit,
    /// The queued deletion wins: publish the tombstone.
    PublishDeletion,
    /// Remove the local record — only ever returned with positive tombstone
    /// evidence.
    DeleteLocal,
    /// Stop publishing this coordinate but change nothing. The safe answer
    /// whenever the head is absent without deletion evidence.
    SuppressPublish,
    /// Needs an explicit human decision; never auto-resolves on a later boot.
    Park(ParkReason),
    /// Not enough information yet. Nothing is written and nothing publishes.
    Defer,
}

/// Why a coordinate was parked, so the resolution surface can explain it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkReason {
    /// Disk differs from the baseline and no head exists — there is no
    /// trustworthy timestamp for the local edit, and file mtime must not be
    /// used to manufacture one.
    LocalEditWithNoHead,
    /// Something is queued at a coordinate this install has no baseline for,
    /// and the relay holds a head. V3.11 cell 2, and the field failure this
    /// whole change exists to stop: a second store with its own fresh
    /// retention database is ALWAYS in this state, so publishing here is
    /// exactly how a stale copy reverts a good edit.
    ///
    /// Covers a queued DELETION as well as a queued edit, and the deletion is
    /// the worse half. An edit that loses this race can be re-made; a deletion
    /// published against a head this install never agreed to destroys content
    /// it has never seen, and the a-tag delete then propagates that removal to
    /// every install that CAN see it.
    NoBaselineWithHead,
}

/// The boot pass's resolution for one coordinate.
///
/// The decision and the fate of the queued publish travel together in one value
/// deliberately. They are not independent: `ApplyHead` and `PublishLocalEdit`
/// both permit publication, so a caller that took the decision and forgot to
/// drop a vacuous queued row would publish the stale copy those decisions were
/// chosen to avoid. One value cannot be half-used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub decision: Decision,
    /// Drop the queued publish: it re-publishes content this install already
    /// agreed was published, so it is a reconcile re-sign with nothing to say.
    pub clear_queued_publish: bool,
}

impl Resolution {
    fn keep(decision: Decision) -> Self {
        Self {
            decision,
            clear_queued_publish: false,
        }
    }
}

/// Decide what to do with one coordinate.
///
/// Gates are evaluated in order and the first match wins. The ordering is the
/// contract: durable local intent is never silently discarded, every
/// destructive outcome is gated behind positive evidence rather than an
/// inference, and no cell publishes on the strength of a lookup that did not
/// complete.
pub fn decide(observation: &Observation) -> Resolution {
    // Gate A — positive deletion evidence, checked first so a coordinate the
    // owner deleted elsewhere is never "restored" by the absent-on-disk cell
    // and never republished by the cells below.
    if observation.tombstone == TombstoneEvidence::Found {
        match &observation.head {
            // A live head overrides the tombstone: a later upsert legitimately
            // re-creates a coordinate, and the head is current relay truth.
            HeadState::Present(_) => {}
            // Deletion is proven and no head contradicts it.
            HeadState::Absent => {
                return Resolution::keep(if observation.disk.is_some() {
                    Decision::DeleteLocal
                } else {
                    // Already gone locally; the tombstone confirms it. Nothing
                    // to do, but it must not be republished.
                    Decision::SuppressPublish
                });
            }
            // The owner deleted this coordinate, but a re-creation reads
            // exactly like this and only a completed head read tells them
            // apart. Nothing publishes and nothing is removed.
            HeadState::LookupFailed => return Resolution::keep(Decision::Defer),
        }
    }

    // Gate B — nothing queued here is provable (V3.11 cells 2 and 3).
    //
    // Ordered ahead of BOTH arbitrations below, and that ordering is what V3.11
    // turns on. With no baseline this install cannot show it ever agreed to
    // anything at the coordinate, so neither its content nor its timestamps are
    // evidence of intent about the relay's state — and a fresh retention
    // database is exactly this state. Gate C's timestamp compare is sound only
    // downstream of here.
    let queued_deletion = observation.queued_deletion_at.is_some();
    if observation.baseline.is_none() && (observation.queued.is_some() || queued_deletion) {
        return Resolution::keep(match &observation.head {
            // Cell 2 — Will's bug, as a cell. A head exists and this install
            // cannot show it ever agreed to anything here, so it cannot claim
            // its queued copy is newer intent. Parked, never auto-resolved.
            HeadState::Present(_) => Decision::Park(ParkReason::NoBaselineWithHead),
            HeadState::Absent => match observation.tombstone {
                // Cell 3 — a create against a provably empty coordinate: a
                // completed, exact, best-available lookup says no head and a
                // completed scan says no tombstone. Nothing to revert and
                // nothing to resurrect, so the genuinely-new record publishes.
                // A queued deletion here removes a purely local coordinate.
                TombstoneEvidence::NotFound => {
                    if queued_deletion {
                        Decision::PublishDeletion
                    } else {
                        Decision::PublishLocalEdit
                    }
                }
                // `Found` already returned at gate A; it is grouped here so the
                // resurrection vector stays closed if that ever changes.
                // `ScanIncomplete` cannot support the absence cell 3 needs.
                TombstoneEvidence::Found | TombstoneEvidence::ScanIncomplete => Decision::Defer,
            },
            HeadState::LookupFailed => Decision::Defer,
        });
    }

    // Gate C — a queued deletion arbitrated against the head by `created_at`
    // (V3.11 cell 4).
    //
    // Sound HERE where V3.2's row 1 was not, and only because of what gate B
    // already excluded: both sides are honest. The tombstone was minted by an
    // interactive delete at a real time and the head's timestamp is the relay's
    // own; neither is the reconcile re-sign that manufactures
    // `max(now, head + 1)`. A store with no baseline never reaches this
    // compare, which is what stops a stale copy from deleting a head it has
    // never seen.
    if let Some(deletion_at) = observation.queued_deletion_at {
        match &observation.head {
            // Deciding a deletion from a lookup that did not complete could
            // wipe a head this device simply failed to see.
            HeadState::LookupFailed => return Resolution::keep(Decision::Defer),
            // Nothing on the relay to contradict the deletion.
            HeadState::Absent => return Resolution::keep(Decision::PublishDeletion),
            // Deletion wins equality: at the same second the removal is the
            // more conservative reading of intent.
            HeadState::Present(head) if deletion_at >= head.created_at => {
                return Resolution::keep(Decision::PublishDeletion)
            }
            // The head is strictly newer than the tombstone, so the coordinate
            // was legitimately re-created after the delete. The tombstone is
            // stale; fall through and decide the live record on its own terms.
            HeadState::Present(_) => {}
        }
    }

    // Gate D — a queued edit resolved against the baseline (V3.11 cells 1a and
    // 1b), NEVER against `created_at`. Gate B returned for every no-baseline
    // queue, so a baseline is present here whenever anything is queued.
    if let (Some(queued), Some(baseline)) =
        (observation.queued.as_ref(), observation.baseline.as_ref())
    {
        if baseline == queued {
            // Cell 1a — the queued event publishes exactly what this install
            // already agreed was published. A reconcile re-sign carrying no new
            // intent, so it is dropped and the coordinate decided as if nothing
            // were queued.
            return Resolution {
                decision: decide_unqueued(observation).decision,
                clear_queued_publish: true,
            };
        }
        // Cell 1b — the queued event differs from the last agreed publish:
        // genuine post-baseline intent, and the only cell that publishes an
        // edit. It still needs a completed lookup, because publishing means
        // re-signing PAST the head and a lookup that did not complete cannot
        // say what the head is. Deferring costs one boot; the row keeps its
        // pending flag and publishes on the next pass that can see the relay.
        return Resolution::keep(match observation.head {
            HeadState::LookupFailed => Decision::Defer,
            _ => Decision::PublishLocalEdit,
        });
    }

    decide_unqueued(observation)
}

/// Decide a coordinate with nothing queued for publish.
///
/// Reached directly when no publish is queued, and from cell 1a once a vacuous
/// queued event has been dropped — the two states are identical from here down,
/// which is why the fall-through is a call rather than a copy of these cells.
fn decide_unqueued(observation: &Observation) -> Resolution {
    let Some(disk) = observation.disk.as_ref() else {
        // Absent on disk. Absence is never deletion intent; that requires a
        // tombstone, handled by gate B.
        return Resolution::keep(match &observation.head {
            HeadState::Present(_) => Decision::RestoreFromRelay,
            // No local record and no head: nothing to reconcile.
            HeadState::Absent => Decision::SuppressPublish,
            HeadState::LookupFailed => Decision::Defer,
        });
    };

    Resolution::keep(match &observation.head {
        // Disk already equals the head. This is the crash-convergence cell:
        // inbound retains the head before writing JSON, so a crash between
        // those two steps lands here and must repair the baseline rather than
        // be misread as a hand edit and republished.
        HeadState::Present(head) if head.projection == *disk => Decision::StampBaseline,

        HeadState::Present(_) => match observation.baseline.as_ref() {
            // Disk is untouched since the baseline, so the head is strictly
            // newer information.
            Some(baseline) if baseline == disk => Decision::ApplyHead,
            // Disk differs from both: a genuine post-baseline edit. Nothing is
            // queued for it yet; the next reconcile queues it and cell 1b
            // publishes it.
            Some(_) => Decision::PublishLocalEdit,
            // No baseline: this install has no provenance for the record, so it
            // cannot claim the difference is an edit.
            None => Decision::Defer,
        },

        // V3.10 — head absent. Suppress, never delete: a wrong absence read
        // costs one boot of deferral, a wrong deletion is unrecoverable.
        HeadState::Absent => match observation.baseline.as_ref() {
            // Disk is untouched and the head is gone. Under V3.2 this deleted
            // locally; V3.10 downgrades it to suppression absent a tombstone.
            Some(baseline) if baseline == disk => Decision::SuppressPublish,
            // A local edit with no head to sign past: no trustworthy edit
            // timestamp exists, so this needs an explicit decision.
            Some(_) => Decision::Park(ParkReason::LocalEditWithNoHead),
            None => Decision::Defer,
        },

        // Nothing is decided from a lookup that did not complete.
        HeadState::LookupFailed => Decision::Defer,
    })
}

#[cfg(test)]
mod tests;
