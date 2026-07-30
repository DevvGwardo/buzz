//! Exact per-coordinate head lookups for the boot decision pass (V3.3).
//!
//! The decision table may only treat a head as ABSENT on the strength of a
//! lookup that (a) named the exact coordinate and (b) completed. Anything
//! weaker is reported as [`HeadState::LookupFailed`], which decides nothing.
//!
//! # Why absence needs its own query shape
//!
//! It is tempting to pull one inventory of all `30175/30176/30177` events for
//! the owner and treat "not in the list" as absent. That is unsound: the
//! effective server-side cap on a query is 1000 rows
//! (`crates/buzz-db/src/event.rs`), so a truncated page is indistinguishable
//! from a short one — absence would be inferred from a cut-off list. The
//! inventory may discover records, but only an exact `{kinds, authors, #d}`
//! lookup may deny them.
//!
//! # Known limitation
//!
//! On relay deployments that route historical reads through a read-replica
//! pool, a replica that has not yet replayed an event returns the same empty
//! page as a genuine deletion, with no staleness budget bounding the
//! difference. The barrier's tracing line is the detection mechanism if this
//! ever fires in the wild; re-adding a `sync_authoritative` opt-in to the
//! protocol is a small standalone change reversible on evidence.

use serde_json::json;

use super::{
    canonical_projection::CanonicalProjection,
    decision::{Head, HeadState},
    retention::RetentionScope,
};
use crate::app_state::AppState;

/// One coordinate to look up: kind plus `d` tag, under the scope's owner.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Coordinate {
    pub kind: u32,
    pub d_tag: String,
}

/// Build the exact filter for one coordinate's head.
///
/// `limit: 2` rather than 1 is deliberate: NIP-33 guarantees at most one head
/// per coordinate, so a second row means the relay disagrees with that
/// assumption and the result must not be trusted as authoritative.
pub fn head_filter(owner_pubkey_hex: &str, coordinate: &Coordinate) -> serde_json::Value {
    json!({
        "kinds": [coordinate.kind],
        "authors": [owner_pubkey_hex],
        "#d": [coordinate.d_tag],
        "limit": 2,
    })
}

/// Interpret the events a head lookup returned for one coordinate.
///
/// Split from the network call so the interpretation rules are testable
/// without a relay. The caller passes only events it actually received; a
/// transport error must become [`HeadState::LookupFailed`] instead of an empty
/// slice, or a network failure would masquerade as a deletion.
pub fn head_state_from_events(coordinate: &Coordinate, events: &[nostr::Event]) -> HeadState {
    // Defense in depth: the relay filtered by kind and `#d`, but a mismatched
    // row here means the answer is about a different coordinate, so it cannot
    // be used to deny this one.
    let matching: Vec<&nostr::Event> = events
        .iter()
        .filter(|event| {
            event.kind.as_u16() as u32 == coordinate.kind
                && event_d_tag(event).as_deref() == Some(coordinate.d_tag.as_str())
        })
        .collect();

    match matching.len() {
        0 => HeadState::Absent,
        1 => HeadState::Present(Head {
            event_id: matching[0].id.to_hex(),
            // Safety: nostr timestamps are seconds and stay below i64::MAX
            // until year 2262 — the same cast the retention store uses.
            created_at: matching[0].created_at.as_secs() as i64,
            projection: CanonicalProjection::from_event(matching[0]),
        }),
        // More than one head for a replaceable coordinate contradicts NIP-33.
        // Refuse to decide rather than pick one.
        _ => HeadState::LookupFailed,
    }
}

/// The `d` tag of an event, if it has exactly one.
fn event_d_tag(event: &nostr::Event) -> Option<String> {
    let mut found: Option<String> = None;
    for tag in event.tags.iter() {
        let parts = tag.as_slice();
        if parts.len() >= 2 && parts[0].as_str() == "d" {
            if found.is_some() {
                // Two `d` tags: the coordinate is ambiguous.
                return None;
            }
            found = Some(parts[1].to_string());
        }
    }
    found
}

/// Look up one coordinate's head on the relay.
///
/// Every failure mode — transport error, malformed response, contradictory
/// result — collapses to [`HeadState::LookupFailed`] so that no caller can
/// mistake a failed lookup for an authoritative absence.
pub async fn fetch_head(
    state: &AppState,
    scope: &RetentionScope,
    api_base_url: &str,
    coordinate: &Coordinate,
) -> HeadState {
    let owner_hex = scope.owner_keys.public_key().to_hex();
    let filter = head_filter(&owner_hex, coordinate);

    match crate::relay::query_relay_at_with_keys(
        state,
        api_base_url,
        std::slice::from_ref(&filter),
        &scope.owner_keys,
        None,
    )
    .await
    {
        Ok(events) => head_state_from_events(coordinate, &events),
        Err(error) => {
            eprintln!(
                "buzz-desktop: config-sync: head lookup failed for {}:{} — {error}",
                coordinate.kind, coordinate.d_tag
            );
            HeadState::LookupFailed
        }
    }
}

#[cfg(test)]
mod tests;
