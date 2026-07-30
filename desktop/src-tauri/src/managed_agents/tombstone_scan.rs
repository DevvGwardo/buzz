//! The exhaustive owner kind:5 scan (V3.3), and the evidence it yields.
//!
//! [`TombstoneEvidence::NotFound`] is a claim that the owner never deleted a
//! coordinate. Under V3.11 that claim is what lets a genuinely new record
//! publish against a provably empty coordinate, and under V3.10 its opposite is
//! the only thing that authorizes deleting a local record. Both directions are
//! consequential, so the claim may only come from a scan that ran to
//! completion — anything less reports [`TombstoneEvidence::ScanIncomplete`],
//! which decides nothing.
//!
//! # Why the scan is exhaustive rather than filtered
//!
//! The obvious query is one `#a`-filtered lookup per coordinate. The relay
//! cannot serve it soundly: `#a` is not a pushed-down constraint
//! (`filter_fully_pushable` in `crates/buzz-relay/src/handlers/req.rs` lists
//! `#a` among the post-filtered keys), so the server applies it AFTER the SQL
//! `LIMIT`. A page that fills with 1000 unrelated kind:5 events returns empty
//! for the coordinate asked about — indistinguishable from no tombstone
//! existing. So the scan pulls the owner's kind:5 history by pages, filters
//! `a`-tags locally, and only reports absence once a page comes back short of
//! its own limit.
//!
//! # Why one scan for every coordinate
//!
//! The scan is per-OWNER, not per-coordinate: one paginated walk yields the
//! evidence for every coordinate the boot pass is about to decide. A
//! per-coordinate scan would multiply the same pages by the number of records
//! on the device for an identical answer.

use std::collections::BTreeSet;

use serde_json::json;

use super::{decision::TombstoneEvidence, head_lookup::Coordinate, retention::RetentionScope};
use crate::app_state::AppState;

/// Page size for the scan. The relay clamps any request to 1000
/// (`crates/buzz-db/src/event.rs`), so asking for more would silently get 1000
/// and make a full page look short.
const SCAN_PAGE_LIMIT: usize = 1000;

/// Hard bound on pages walked in one boot, so a pathological history cannot
/// stall startup indefinitely. Hitting it yields an INCOMPLETE scan, never a
/// premature `NotFound`.
const MAX_SCAN_PAGES: usize = 20;

/// Which coordinates the owner has tombstoned, from a scan that either ran to
/// completion or did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TombstonedCoordinates {
    tombstoned: BTreeSet<(u32, String)>,
    complete: bool,
}

impl TombstonedCoordinates {
    /// The result of a scan that did not finish. Every lookup against it
    /// reports [`TombstoneEvidence::ScanIncomplete`], even for coordinates the
    /// partial results did name — a partial scan cannot support `NotFound`, and
    /// reporting `Found` from it would make the evidence's meaning depend on
    /// which page a tombstone happened to land on.
    pub fn incomplete() -> Self {
        Self {
            tombstoned: BTreeSet::new(),
            complete: false,
        }
    }

    /// The evidence this scan provides about one coordinate.
    pub fn evidence(&self, coordinate: &Coordinate) -> TombstoneEvidence {
        if !self.complete {
            return TombstoneEvidence::ScanIncomplete;
        }
        if self
            .tombstoned
            .contains(&(coordinate.kind, coordinate.d_tag.clone()))
        {
            TombstoneEvidence::Found
        } else {
            TombstoneEvidence::NotFound
        }
    }
}

/// One page of the owner's kind:5 history, oldest-bound by a keyset cursor.
///
/// # Known limitation
///
/// On relay deployments that route historical reads through a read-replica
/// pool, a replica that has not yet replayed a tombstone answers "not found"
/// in the dangerous direction. The barrier's tracing line surfaces this if
/// it ever fires; re-adding `sync_authoritative` to the protocol is a small
/// standalone change reversible on observed evidence.
pub fn scan_page_filter(owner_pubkey_hex: &str, cursor: Option<&PageCursor>) -> serde_json::Value {
    let mut filter = json!({
        "kinds": [buzz_core_pkg::kind::KIND_DELETION],
        "authors": [owner_pubkey_hex],
        "limit": SCAN_PAGE_LIMIT,
    });
    if let Some(cursor) = cursor {
        // Composite keyset cursor: the relay requires both halves or neither,
        // and rejects `before_id` without `until`. A timestamp-only cursor
        // would drop every event tied at the page boundary's second.
        filter["until"] = json!(cursor.until);
        filter["before_id"] = json!(cursor.before_id);
    }
    filter
}

/// The `(created_at, id)` pair that bounds the next page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    pub until: i64,
    pub before_id: String,
}

/// The cursor for the page after `events`, or `None` when this page ends the
/// walk.
///
/// A page shorter than the limit it asked for is the ONLY proof of exhaustion.
/// `rows == limit` never proves there is more, but it never proves there isn't
/// either, so the walk continues until a page comes back short.
pub fn next_cursor(events: &[nostr::Event]) -> Option<PageCursor> {
    if events.len() < SCAN_PAGE_LIMIT {
        return None;
    }
    let last = events.last()?;
    Some(PageCursor {
        until: last.created_at.as_secs() as i64,
        before_id: last.id.to_hex(),
    })
}

/// The config coordinates a page of kind:5 events tombstones.
///
/// `a`-tags are parsed locally with the same owner-scoping rule the inbound
/// tombstone path uses: only the coordinate's own author may tombstone it, so
/// a validly signed kind:5 naming another owner's coordinate names nothing
/// here. Coordinates outside the three config kinds are ignored — the owner's
/// kind:5 history includes deletions this pass has no opinion about.
pub fn tombstoned_in_page(events: &[nostr::Event]) -> BTreeSet<(u32, String)> {
    use super::canonical_projection::CanonicalProjection;

    events
        .iter()
        .filter_map(|event| {
            let (kind, d_tag) = deletion_coordinate(event)?;
            CanonicalProjection::is_config_kind(kind).then_some((kind, d_tag))
        })
        .collect()
}

/// Parse a NIP-09 `a`-tag coordinate `<kind>:<owner_pubkey>:<d_tag>` into its
/// target kind and d-tag, rejecting a coordinate the event's author does not
/// own.
fn deletion_coordinate(event: &nostr::Event) -> Option<(u32, String)> {
    event.tags.iter().find_map(|tag| {
        let values = tag.as_slice();
        if values.first().map(|v| v.as_str()) != Some("a") {
            return None;
        }
        // `<kind>:<owner>:<d_tag>` — the d-tag may itself contain ':', so split
        // at most twice and keep the remainder whole.
        let mut parts = values.get(1)?.splitn(3, ':');
        let kind: u32 = parts.next()?.parse().ok()?;
        if parts.next()? != event.pubkey.to_hex() {
            return None;
        }
        Some((kind, parts.next()?.to_string()))
    })
}

/// Walk the owner's kind:5 history to exhaustion.
///
/// Every failure mode — a transport error, a page that never ends — yields
/// [`TombstonedCoordinates::incomplete`], so a scan that could not prove
/// absence never claims it.
pub async fn scan_tombstones(
    state: &AppState,
    scope: &RetentionScope,
    api_base_url: &str,
) -> TombstonedCoordinates {
    let owner_hex = scope.owner_keys.public_key().to_hex();
    let mut tombstoned = BTreeSet::new();
    let mut cursor: Option<PageCursor> = None;

    for _ in 0..MAX_SCAN_PAGES {
        let filter = scan_page_filter(&owner_hex, cursor.as_ref());
        let events = match crate::relay::query_relay_at_with_keys(
            state,
            api_base_url,
            std::slice::from_ref(&filter),
            &scope.owner_keys,
            None,
        )
        .await
        {
            Ok(events) => events,
            Err(error) => {
                tracing::warn!(
                    target: "buzz::config_sync",
                    %error,
                    "tombstone scan page failed; evidence is incomplete"
                );
                return TombstonedCoordinates::incomplete();
            }
        };

        tombstoned.extend(tombstoned_in_page(&events));
        match next_cursor(&events) {
            Some(next) => cursor = Some(next),
            None => {
                tracing::info!(
                    target: "buzz::config_sync",
                    tombstoned = tombstoned.len(),
                    "tombstone scan complete"
                );
                return TombstonedCoordinates {
                    tombstoned,
                    complete: true,
                };
            }
        }
    }

    tracing::warn!(
        target: "buzz::config_sync",
        max_pages = MAX_SCAN_PAGES,
        "tombstone scan hit its page bound; evidence is incomplete"
    );
    TombstonedCoordinates::incomplete()
}

#[cfg(test)]
mod tests;
