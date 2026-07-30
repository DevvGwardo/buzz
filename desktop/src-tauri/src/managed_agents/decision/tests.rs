use super::*;
use crate::managed_agents::config_sync::{blocks_publication, blocks_tombstone_publication};

fn projection(content: &str) -> CanonicalProjection {
    CanonicalProjection {
        content: content.to_string(),
        shared: false,
    }
}

/// A relay head carrying `content` at `created_at`. The event id is derived
/// from the content so two heads with different content never collide on id.
fn head(content: &str, created_at: i64) -> Head {
    Head {
        event_id: format!("id-{content}"),
        created_at,
        projection: projection(content),
    }
}

/// A coordinate whose disk, baseline, and head all agree, with nothing queued —
/// the steady state. Individual tests mutate one axis at a time from here.
fn settled() -> Observation {
    Observation {
        disk: Some(projection("v1")),
        queued: None,
        head: HeadState::Present(head("v1", 100)),
        baseline: Some(projection("v1")),
        queued_deletion_at: None,
        tombstone: TombstoneEvidence::NotFound,
    }
}

/// Every state the table can be asked about, as a cross product of each axis.
///
/// The sweeps below are the only proofs that hold over states nobody thought
/// to write down, which is where this table's failures have actually come
/// from — the reverted-config bug was a cell no row-by-row test covered.
/// Enumerating is cheap (under a thousand states) and the coverage is total.
fn all_observations() -> impl Iterator<Item = Observation> {
    let disks = [None, Some(projection("v1")), Some(projection("edited"))];
    let queues = [None, Some(projection("v1")), Some(projection("edited"))];
    let heads = [
        HeadState::Present(head("v1", 100)),
        HeadState::Present(head("v2", 200)),
        HeadState::Absent,
        HeadState::LookupFailed,
    ];
    let baselines = [None, Some(projection("v1")), Some(projection("old"))];
    // One tombstone older than every head and one newer, so the cell-4
    // arbitration is exercised in both directions wherever it is reachable.
    let deletions = [None, Some(50), Some(500)];
    let tombstones = [
        TombstoneEvidence::Found,
        TombstoneEvidence::NotFound,
        TombstoneEvidence::ScanIncomplete,
    ];

    disks.into_iter().flat_map(move |disk| {
        let queues = queues.clone();
        let heads = heads.clone();
        let baselines = baselines.clone();
        queues.into_iter().flat_map(move |queued| {
            let heads = heads.clone();
            let baselines = baselines.clone();
            let disk = disk.clone();
            heads.into_iter().flat_map(move |head| {
                let baselines = baselines.clone();
                let disk = disk.clone();
                let queued = queued.clone();
                baselines.into_iter().flat_map(move |baseline| {
                    let disk = disk.clone();
                    let queued = queued.clone();
                    let head = head.clone();
                    deletions.into_iter().flat_map(move |queued_deletion_at| {
                        let disk = disk.clone();
                        let queued = queued.clone();
                        let head = head.clone();
                        let baseline = baseline.clone();
                        tombstones.into_iter().map(move |tombstone| Observation {
                            disk: disk.clone(),
                            queued: queued.clone(),
                            head: head.clone(),
                            baseline: baseline.clone(),
                            queued_deletion_at,
                            tombstone,
                        })
                    })
                })
            })
        })
    })
}

// ── The invariants ─────────────────────────────────────────────────────────

/// **Will's bug, stated as a property.** This is the whole point of V3.11.
///
/// A store with no baseline for a coordinate cannot show it ever agreed to
/// anything published there. When the relay nonetheless holds a head, that
/// store's queued content is by definition uninformed about the head — which
/// is exactly the state of a second install with a fresh retention database.
/// Its `created_at` is manufactured by `monotonic_created_at = max(now,
/// head + 1)`, so any comparison decided on it is one the stale store always
/// wins. Nothing may reach the relay from this state, for any combination of
/// the remaining axes.
///
/// Asserted against the real publication gates rather than against the
/// `Decision` enum, because the gate is what the flush loop obeys: a decision
/// that suppresses but is enforced on the wrong row is indistinguishable from
/// no gate at all.
///
/// The statement is over BOTH rows and is unconditional on what is queued. A
/// coordinate's live row and its tombstone row have different primary keys, so
/// a single-row invariant is provably insufficient — and the assertion holds
/// even where nothing is queued right now, because the gate this pass leaves
/// behind is what governs a row queued after it.
#[test]
fn test_no_baseline_with_a_live_head_releases_neither_row() {
    let mut covered = 0;
    for observation in all_observations() {
        if observation.baseline.is_some() || !matches!(observation.head, HeadState::Present(_)) {
            continue;
        }
        let resolution = decide(&observation);

        assert!(
            blocks_publication(&resolution),
            "the live row escaped the gate with no baseline against a live head: \
             {observation:?} -> {resolution:?}"
        );
        assert!(
            blocks_tombstone_publication(&resolution),
            "the tombstone row escaped the gate with no baseline against a live head: \
             {observation:?} -> {resolution:?}"
        );
        covered += 1;
    }
    assert!(
        covered > 0,
        "the sweep must actually reach the guarded states"
    );
}

/// The cell-2 decision behind that property, named so a future reader can see
/// which cell the sweep is protecting.
#[test]
fn test_no_baseline_with_a_live_head_parks() {
    for queued in [Some(projection("stale")), None] {
        for queued_deletion_at in [None, Some(500)] {
            if queued.is_none() && queued_deletion_at.is_none() {
                continue; // nothing queued: not cell 2
            }
            let observation = Observation {
                disk: Some(projection("stale")),
                queued: queued.clone(),
                head: HeadState::Present(head("good-edit", 200)),
                baseline: None,
                queued_deletion_at,
                tombstone: TombstoneEvidence::NotFound,
            };
            assert_eq!(
                decide(&observation).decision,
                Decision::Park(ParkReason::NoBaselineWithHead),
                "cell 2 must park, not auto-resolve: {observation:?}"
            );
        }
    }
}

/// `created_at` is never consulted for a live queued row against a head.
///
/// The head's timestamp is varied across the whole range around the queued
/// row's while every other axis is held fixed. If any decision moved, some
/// cell would be arbitrating on a value the reconcile manufactures, and the
/// stale store would win it.
///
/// Deliberately excludes queued deletions: cell 4 arbitrates those on
/// `created_at` by design, and soundly, because a tombstone is only ever
/// minted by an interactive delete.
#[test]
fn test_head_created_at_never_moves_a_live_decision() {
    for observation in all_observations() {
        if observation.queued_deletion_at.is_some() {
            continue; // cell 4 consults timestamps on purpose
        }
        let HeadState::Present(present) = &observation.head else {
            continue;
        };
        let baseline = decide(&observation);
        for created_at in [1, 99, 100, 101, 1_000_000] {
            let shifted = Observation {
                head: HeadState::Present(Head {
                    created_at,
                    ..present.clone()
                }),
                ..observation.clone()
            };
            assert_eq!(
                decide(&shifted),
                baseline,
                "the head's created_at moved a live decision: {observation:?} at {created_at}"
            );
        }
    }
}

/// No decision path may destroy local state without positive tombstone
/// evidence. The safety property of the whole table, over the full state space
/// rather than row by row: for a managed agent the removal is irreversible,
/// since the relay projection is a strict subset of the local record.
#[test]
fn test_deletion_is_unreachable_without_tombstone_evidence() {
    for observation in all_observations() {
        if observation.tombstone == TombstoneEvidence::Found {
            continue;
        }
        assert_ne!(
            decide(&observation).decision,
            Decision::DeleteLocal,
            "reached DeleteLocal without evidence: {observation:?}"
        );
    }
}

/// Nothing is decided from a lookup that did not complete. A flaky network
/// must never present as a deletion or as an absence worth publishing into.
///
/// Scoped to the DECISION. Dropping a vacuous queued publish is deliberately
/// still allowed here: "the queued row says exactly what the baseline says" is
/// a purely local fact that needs no relay evidence, and the row could never
/// become publishable anyway. `test_a_queued_publish_is_only_dropped_when_it_\
/// equals_the_baseline` is what bounds that drop.
#[test]
fn test_a_failed_head_lookup_decides_nothing() {
    for observation in all_observations() {
        if observation.head != HeadState::LookupFailed {
            continue;
        }
        assert_eq!(
            decide(&observation).decision,
            Decision::Defer,
            "a failed lookup must decide nothing: {observation:?}"
        );
    }
}

/// A queued publish is only ever DROPPED as vacuous, never as a judgement
/// about content. Dropping it anywhere else would silently discard durable
/// local intent, which is the one thing row 1 has always guaranteed against.
#[test]
fn test_a_queued_publish_is_only_dropped_when_it_equals_the_baseline() {
    for observation in all_observations() {
        if !decide(&observation).clear_queued_publish {
            continue;
        }
        assert_eq!(
            observation.queued, observation.baseline,
            "dropped a queued publish that carried new intent: {observation:?}"
        );
        assert!(observation.queued.is_some());
    }
}

// ── Cell 1: a queued row against its baseline ──────────────────────────────

/// Cell 1a. The queued event publishes exactly what this install already
/// agreed was published, so it is a reconcile re-sign carrying nothing. It is
/// dropped rather than gated: it can never become publishable, so gating would
/// hold it forever.
#[test]
fn test_vacuous_re_sign_is_dropped_and_falls_through() {
    let observation = Observation {
        disk: Some(projection("v1")),
        queued: Some(projection("v1")),
        head: HeadState::Present(head("v2", 200)),
        baseline: Some(projection("v1")),
        queued_deletion_at: None,
        tombstone: TombstoneEvidence::NotFound,
    };
    let resolution = decide(&observation);
    assert!(resolution.clear_queued_publish);
    // Fell through to the unqueued cells, which see disk untouched since the
    // baseline and adopt the newer head.
    assert_eq!(resolution.decision, Decision::ApplyHead);
}

/// Cell 1b. An edit made after the last agreed publish is real user intent and
/// must still reach the relay — suppression cannot simply block anything
/// queued.
#[test]
fn test_queued_edit_past_the_baseline_publishes() {
    let observation = Observation {
        disk: Some(projection("my-edit")),
        queued: Some(projection("my-edit")),
        head: HeadState::Present(head("v1", 100)),
        baseline: Some(projection("v1")),
        queued_deletion_at: None,
        tombstone: TombstoneEvidence::NotFound,
    };
    let resolution = decide(&observation);
    assert_eq!(resolution.decision, Decision::PublishLocalEdit);
    assert!(!blocks_publication(&resolution));
}

/// The comparand is the QUEUED event, not disk. The flush loop publishes the
/// queued row's bytes, so a divergence between disk and the queued row must be
/// decided on what actually goes out. Here disk still equals the baseline
/// while the queued row does not — the queued row wins the comparison.
#[test]
fn test_arbitration_reads_the_queued_row_not_disk() {
    let observation = Observation {
        disk: Some(projection("v1")),
        queued: Some(projection("diverged")),
        head: HeadState::Present(head("v1", 100)),
        baseline: Some(projection("v1")),
        queued_deletion_at: None,
        tombstone: TombstoneEvidence::NotFound,
    };
    assert_eq!(decide(&observation).decision, Decision::PublishLocalEdit);
}

// ── Cell 3: the evidence-gated carve-out ───────────────────────────────────

/// A create against a provably empty coordinate: a completed, exact,
/// best-available (replica-lag-exposed) lookup says no head and a completed
/// scan says no tombstone. Nothing to
/// revert and nothing to resurrect, so a genuinely new agent publishes rather
/// than parking forever.
#[test]
fn test_new_coordinate_with_both_evidence_gates_publishes() {
    let observation = Observation {
        disk: Some(projection("brand-new")),
        queued: Some(projection("brand-new")),
        head: HeadState::Absent,
        baseline: None,
        queued_deletion_at: None,
        tombstone: TombstoneEvidence::NotFound,
    };
    let resolution = decide(&observation);
    assert_eq!(resolution.decision, Decision::PublishLocalEdit);
    assert!(!blocks_publication(&resolution));
}

/// The resurrection vector. A deleted coordinate reads head-absent exactly
/// like a new one, and the relay keeps no watermark — so without the tombstone
/// gate, cell 3 would republish every record the owner ever deleted.
#[test]
fn test_a_deleted_coordinate_is_never_republished_as_new() {
    let observation = Observation {
        disk: Some(projection("deleted-agent")),
        queued: Some(projection("deleted-agent")),
        head: HeadState::Absent,
        baseline: None,
        queued_deletion_at: None,
        tombstone: TombstoneEvidence::Found,
    };
    let resolution = decide(&observation);
    assert_eq!(resolution.decision, Decision::DeleteLocal);
    assert!(blocks_publication(&resolution));
}

/// An incomplete scan cannot support the absence cell 3 needs, so the queued
/// row is held rather than published on unproven evidence.
#[test]
fn test_new_coordinate_defers_on_an_incomplete_scan() {
    let observation = Observation {
        disk: Some(projection("brand-new")),
        queued: Some(projection("brand-new")),
        head: HeadState::Absent,
        baseline: None,
        queued_deletion_at: None,
        tombstone: TombstoneEvidence::ScanIncomplete,
    };
    assert_eq!(decide(&observation).decision, Decision::Defer);
}

// ── Cell 4: tombstone-vs-live arbitration ──────────────────────────────────

/// Downstream of the baseline gates, both timestamps are honest — the
/// tombstone's was minted by an interactive delete, the head's by the relay —
/// so arbitrating on them is sound. Deletion wins equality: at the same second
/// the removal is the more conservative reading of intent.
#[test]
fn test_a_tombstone_at_or_after_the_head_publishes_the_deletion() {
    for deletion_at in [100, 200] {
        let observation = Observation {
            queued_deletion_at: Some(deletion_at),
            ..settled()
        };
        let resolution = decide(&observation);
        assert_eq!(resolution.decision, Decision::PublishDeletion);
        // The tombstone row is released and the live row it deletes is
        // withheld, so the record cannot race its own deletion.
        assert!(!blocks_tombstone_publication(&resolution));
        assert!(blocks_publication(&resolution));
    }
}

/// A head strictly newer than the tombstone means the coordinate was
/// legitimately re-created after the delete. The tombstone is stale and the
/// live record is decided on its own terms.
#[test]
fn test_a_head_newer_than_the_tombstone_survives_it() {
    let observation = Observation {
        queued_deletion_at: Some(50),
        ..settled()
    };
    let resolution = decide(&observation);
    assert_eq!(resolution.decision, Decision::StampBaseline);
    assert!(blocks_tombstone_publication(&resolution));
}

/// Deciding a deletion from a lookup that did not complete could wipe a head
/// this device simply failed to see.
#[test]
fn test_a_deletion_behind_a_failed_lookup_defers() {
    let observation = Observation {
        head: HeadState::LookupFailed,
        queued_deletion_at: Some(500),
        ..settled()
    };
    assert_eq!(decide(&observation).decision, Decision::Defer);
}

// ── V3.10: suppression is not deletion ─────────────────────────────────────

/// The core V3.10 rule. Relay-side history loss under one URL presents as
/// every coordinate being head-absent at once; under a delete-on-absence rule
/// that wipes the device.
#[test]
fn test_head_absent_with_clean_disk_suppresses_instead_of_deleting() {
    let observation = Observation {
        head: HeadState::Absent,
        ..settled()
    };
    let resolution = decide(&observation);
    assert_eq!(resolution.decision, Decision::SuppressPublish);
    assert!(blocks_publication(&resolution));
}

/// A live head is current relay truth: a later upsert legitimately re-creates
/// a coordinate, so an older tombstone must not delete it.
#[test]
fn test_live_head_overrides_an_older_tombstone() {
    let observation = Observation {
        tombstone: TombstoneEvidence::Found,
        ..settled()
    };
    assert_eq!(decide(&observation).decision, Decision::StampBaseline);
}

/// Proven deletion plus a head read that never completed is ambiguous: a
/// re-creation reads the same way. Neither removing nor republishing is safe.
#[test]
fn test_tombstone_with_a_failed_lookup_defers() {
    let observation = Observation {
        head: HeadState::LookupFailed,
        tombstone: TombstoneEvidence::Found,
        ..settled()
    };
    assert_eq!(decide(&observation).decision, Decision::Defer);
}

// ── Crash convergence and the unqueued cells ───────────────────────────────

/// Inbound retains the head BEFORE writing JSON, so a crash in between leaves
/// disk equal to the head but unequal to the old baseline. That must repair
/// the baseline, not be misread as a hand edit and republished.
#[test]
fn test_disk_equal_to_head_stamps_baseline_even_when_baseline_is_stale() {
    let observation = Observation {
        disk: Some(projection("v2")),
        head: HeadState::Present(head("v2", 200)),
        baseline: Some(projection("v1")),
        ..settled()
    };
    assert_eq!(decide(&observation).decision, Decision::StampBaseline);
}

/// The same cell with no baseline at all. This is the bootstrap: without it
/// the baseline stays `None` forever and every baseline-dependent cell is
/// unreachable.
#[test]
fn test_disk_equal_to_head_stamps_baseline_when_baseline_is_missing() {
    let observation = Observation {
        disk: Some(projection("v2")),
        head: HeadState::Present(head("v2", 200)),
        baseline: None,
        ..settled()
    };
    assert_eq!(decide(&observation).decision, Decision::StampBaseline);
}

/// Disk is untouched since the baseline, so the differing head is strictly
/// newer information and wins.
#[test]
fn test_untouched_disk_adopts_a_differing_head() {
    let observation = Observation {
        disk: Some(projection("v1")),
        head: HeadState::Present(head("v2", 200)),
        baseline: Some(projection("v1")),
        ..settled()
    };
    assert_eq!(decide(&observation).decision, Decision::ApplyHead);
}

/// Without a baseline the difference cannot be attributed to a local edit, so
/// nothing is adopted and nothing is published.
#[test]
fn test_no_baseline_never_acts_on_a_difference() {
    let observation = Observation {
        disk: Some(projection("stale")),
        head: HeadState::Present(head("current", 200)),
        baseline: None,
        ..settled()
    };
    assert_eq!(decide(&observation).decision, Decision::Defer);
}

/// Absence on disk is never deletion intent; that requires a tombstone.
#[test]
fn test_missing_disk_record_with_live_head_restores() {
    let observation = Observation {
        disk: None,
        ..settled()
    };
    assert_eq!(decide(&observation).decision, Decision::RestoreFromRelay);
}

#[test]
fn test_missing_disk_record_and_missing_head_is_a_no_op() {
    let observation = Observation {
        disk: None,
        head: HeadState::Absent,
        ..settled()
    };
    assert_eq!(decide(&observation).decision, Decision::SuppressPublish);
}

/// A local edit with no head has no trustworthy timestamp to sign past, and
/// file mtime must not be used to manufacture one — so it needs a human.
#[test]
fn test_local_edit_with_no_head_parks() {
    let observation = Observation {
        disk: Some(projection("edited")),
        head: HeadState::Absent,
        baseline: Some(projection("v1")),
        ..settled()
    };
    assert_eq!(
        decide(&observation).decision,
        Decision::Park(ParkReason::LocalEditWithNoHead)
    );
}
