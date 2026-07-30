use super::*;
use buzz_core_pkg::kind::{KIND_DELETION, KIND_MANAGED_AGENT, KIND_PERSONA, KIND_TEAM};
use nostr::{EventBuilder, Keys, Kind, Tag};

/// A signed kind:5 carrying one `a`-tag, with control over whose coordinate it
/// names — the owner-scoping rule is what makes a foreign tombstone inert.
fn deletion_of(keys: &Keys, coordinate_owner_hex: &str, kind: u32, d_tag: &str) -> nostr::Event {
    let coord = format!("{kind}:{coordinate_owner_hex}:{d_tag}");
    EventBuilder::new(Kind::Custom(KIND_DELETION as u16), "")
        .tags(vec![Tag::parse(["a", coord.as_str()]).unwrap()])
        .sign_with_keys(keys)
        .unwrap()
}

/// A kind:5 deleting by event id instead of coordinate. It names no
/// parameterized coordinate at all.
fn deletion_by_event_id(keys: &Keys) -> nostr::Event {
    EventBuilder::new(Kind::Custom(KIND_DELETION as u16), "")
        .tags(vec![Tag::parse(["e", &"ab".repeat(32)]).unwrap()])
        .sign_with_keys(keys)
        .unwrap()
}

fn coordinate(kind: u32, d_tag: &str) -> Coordinate {
    Coordinate {
        kind,
        d_tag: d_tag.to_string(),
    }
}

// ── The evidence contract ──────────────────────────────────────────────────

/// An incomplete scan decides nothing. Reporting `Found` from partial results
/// would make the evidence's meaning depend on which page a tombstone landed
/// on, and reporting `NotFound` would be the unsound direction outright.
#[test]
fn test_an_incomplete_scan_reports_no_evidence_for_any_coordinate() {
    let scan = TombstonedCoordinates::incomplete();

    assert_eq!(
        scan.evidence(&coordinate(KIND_PERSONA, "anything")),
        TombstoneEvidence::ScanIncomplete
    );
}

#[test]
fn test_a_complete_scan_distinguishes_tombstoned_from_untouched() {
    let keys = Keys::generate();
    let owner = keys.public_key().to_hex();
    let events = [deletion_of(&keys, &owner, KIND_PERSONA, "deleted")];

    let scan = TombstonedCoordinates {
        tombstoned: tombstoned_in_page(&events),
        complete: true,
    };

    assert_eq!(
        scan.evidence(&coordinate(KIND_PERSONA, "deleted")),
        TombstoneEvidence::Found
    );
    assert_eq!(
        scan.evidence(&coordinate(KIND_PERSONA, "never-deleted")),
        TombstoneEvidence::NotFound
    );
    // Same d-tag under a different kind is a different coordinate.
    assert_eq!(
        scan.evidence(&coordinate(KIND_TEAM, "deleted")),
        TombstoneEvidence::NotFound
    );
}

// ── Parsing a page into coordinates ────────────────────────────────────────

/// Only the coordinate's own author may tombstone it. A validly signed kind:5
/// naming someone else's coordinate is inert, or one owner could suppress
/// another's config by publishing a deletion for it.
#[test]
fn test_a_tombstone_for_another_owners_coordinate_names_nothing() {
    let keys = Keys::generate();
    let stranger = Keys::generate().public_key().to_hex();

    let tombstoned = tombstoned_in_page(&[deletion_of(&keys, &stranger, KIND_PERSONA, "victim")]);

    assert!(
        tombstoned.is_empty(),
        "a foreign coordinate must not be treated as deleted: {tombstoned:?}"
    );
}

/// The owner's kind:5 history covers deletions this pass has no opinion about.
#[test]
fn test_deletions_outside_the_config_kinds_are_ignored() {
    let keys = Keys::generate();
    let owner = keys.public_key().to_hex();

    let tombstoned = tombstoned_in_page(&[
        deletion_of(&keys, &owner, 30023, "some-article"),
        deletion_of(&keys, &owner, KIND_MANAGED_AGENT, "agent"),
    ]);

    assert_eq!(
        tombstoned,
        BTreeSet::from([(KIND_MANAGED_AGENT, "agent".to_string())])
    );
}

/// A d-tag may itself contain `:`, so the coordinate splits at most twice and
/// keeps the remainder whole. Truncating here would report evidence about a
/// coordinate that does not exist while missing the one that does.
#[test]
fn test_a_d_tag_containing_colons_survives_parsing() {
    let keys = Keys::generate();
    let owner = keys.public_key().to_hex();

    let tombstoned = tombstoned_in_page(&[deletion_of(&keys, &owner, KIND_PERSONA, "a:b:c")]);

    assert_eq!(
        tombstoned,
        BTreeSet::from([(KIND_PERSONA, "a:b:c".to_string())])
    );
}

#[test]
fn test_an_event_id_deletion_names_no_coordinate() {
    let keys = Keys::generate();

    assert!(tombstoned_in_page(&[deletion_by_event_id(&keys)]).is_empty());
}

// ── Pagination ─────────────────────────────────────────────────────────────

/// A short page is the ONLY proof the walk is exhausted.
#[test]
fn test_a_short_page_ends_the_walk() {
    let keys = Keys::generate();
    let owner = keys.public_key().to_hex();

    assert_eq!(
        next_cursor(&[deletion_of(&keys, &owner, KIND_PERSONA, "one")]),
        None
    );
    assert_eq!(next_cursor(&[]), None);
}

/// A page that exactly fills its limit proves nothing about exhaustion, so the
/// walk continues from the oldest event on it.
#[test]
fn test_a_full_page_continues_from_its_oldest_event() {
    let keys = Keys::generate();
    let owner = keys.public_key().to_hex();
    let filler = deletion_of(&keys, &owner, KIND_PERSONA, "filler");
    let oldest = deletion_of(&keys, &owner, KIND_PERSONA, "oldest");

    let mut page = vec![filler; SCAN_PAGE_LIMIT - 1];
    page.push(oldest.clone());

    assert_eq!(
        next_cursor(&page),
        Some(PageCursor {
            until: oldest.created_at.as_secs() as i64,
            before_id: oldest.id.to_hex(),
        }),
        "the cursor must be the composite keyset of the page's last event"
    );
}

// ── The page filter ────────────────────────────────────────────────────────

/// The page filter names the owner's kind:5 events with the SCAN_PAGE_LIMIT
/// cap. The scan is per-owner (not per-coordinate) so one walk covers every
/// coordinate the barrier is deciding on.
#[test]
fn test_page_filter_covers_owner_deletions() {
    let with_cursor = scan_page_filter(
        "ownerhex",
        Some(&PageCursor {
            until: 100,
            before_id: "abc".to_string(),
        }),
    );

    for filter in [scan_page_filter("ownerhex", None), with_cursor] {
        assert_eq!(filter["kinds"], serde_json::json!([KIND_DELETION]));
        assert_eq!(filter["authors"], serde_json::json!(["ownerhex"]));
    }
}

/// The relay rejects `before_id` without `until` and drops events tied at the
/// boundary second given `until` alone, so the cursor is all-or-nothing.
#[test]
fn test_the_cursor_halves_are_both_present_or_both_absent() {
    let first = scan_page_filter("ownerhex", None);
    assert!(first.get("until").is_none());
    assert!(first.get("before_id").is_none());

    let next = scan_page_filter(
        "ownerhex",
        Some(&PageCursor {
            until: 100,
            before_id: "abc".to_string(),
        }),
    );
    assert_eq!(next["until"], serde_json::json!(100));
    assert_eq!(next["before_id"], serde_json::json!("abc"));
}

/// Asking for more than the relay's clamp would silently get the clamp back
/// and make a full page look short — which is exactly the state that ends the
/// walk and licenses a `NotFound`.
#[test]
fn test_the_page_limit_does_not_exceed_the_relay_clamp() {
    /// The relay's own clamp (`crates/buzz-db/src/event.rs`).
    const RELAY_CLAMP: u64 = 1000;

    let limit = scan_page_filter("ownerhex", None)["limit"]
        .as_u64()
        .expect("the filter must carry a numeric limit");

    assert_eq!(limit, SCAN_PAGE_LIMIT as u64);
    assert!(
        limit <= RELAY_CLAMP,
        "asking for {limit} would silently get {RELAY_CLAMP} back and make a full page look short"
    );
}
