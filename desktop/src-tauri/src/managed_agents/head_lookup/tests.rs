use super::*;
use crate::managed_agents::persona_events::build_persona_event;
use buzz_core_pkg::kind::{KIND_PERSONA, KIND_TEAM};

fn coordinate() -> Coordinate {
    Coordinate {
        kind: KIND_PERSONA,
        d_tag: "test-persona".to_string(),
    }
}

fn signed_persona(keys: &nostr::Keys, d_tag: &str, shared: bool) -> nostr::Event {
    use std::collections::BTreeMap;
    let record = crate::managed_agents::AgentDefinition {
        id: d_tag.to_string(),
        display_name: "Test".to_string(),
        avatar_url: None,
        system_prompt: "prompt".to_string(),
        runtime: None,
        model: None,
        provider: None,
        name_pool: Vec::new(),
        is_builtin: false,
        is_active: true,
        shared,
        source_team: None,
        source_team_persona_slug: None,
        catalog_source: None,
        env_vars: BTreeMap::new(),
        respond_to: None,
        respond_to_allowlist: Vec::new(),
        parallelism: None,
        created_at: "2025-01-01T00:00:00Z".to_string(),
        updated_at: "2025-01-01T00:00:00Z".to_string(),
    };
    build_persona_event(&record)
        .unwrap()
        .sign_with_keys(keys)
        .unwrap()
}

/// The filter names the exact coordinate (kinds + authors + #d) and requests
/// limit:2 so a NIP-33 violation is detectable. This shape is what makes the
/// absence claim sound — an inventory query cannot bound absence.
#[test]
fn test_head_filter_is_exact() {
    let filter = head_filter("ownerhex", &coordinate());

    assert_eq!(filter["kinds"], serde_json::json!([KIND_PERSONA]));
    assert_eq!(filter["authors"], serde_json::json!(["ownerhex"]));
    assert_eq!(filter["#d"], serde_json::json!(["test-persona"]));
}

/// A cap of 1 could not tell "exactly one head" from "the relay truncated the
/// page", so the lookup asks for one more than NIP-33 permits.
#[test]
fn test_head_filter_can_observe_a_nip33_violation() {
    assert_eq!(head_filter("ownerhex", &coordinate())["limit"], 2);
}

#[test]
fn test_empty_result_is_absent() {
    assert_eq!(
        head_state_from_events(&coordinate(), &[]),
        HeadState::Absent
    );
}

#[test]
fn test_single_matching_event_is_present_with_its_projection() {
    let keys = nostr::Keys::generate();
    let event = signed_persona(&keys, "test-persona", false);

    assert_eq!(
        head_state_from_events(&coordinate(), std::slice::from_ref(&event)),
        HeadState::Present(Head {
            event_id: event.id.to_hex(),
            created_at: event.created_at.as_secs() as i64,
            projection: CanonicalProjection::from_event(&event),
        })
    );
}

/// The `shared` tag has to survive the lookup, or a disk-vs-head compare would
/// report a spurious difference for a shared persona and republish it forever.
#[test]
fn test_present_head_preserves_the_shared_tag() {
    let keys = nostr::Keys::generate();
    let event = signed_persona(&keys, "test-persona", true);

    match head_state_from_events(&coordinate(), std::slice::from_ref(&event)) {
        HeadState::Present(head) => assert!(head.projection.shared),
        other => panic!("expected Present, got {other:?}"),
    }
}

/// Two heads for one replaceable coordinate contradicts NIP-33. Picking one
/// would make the decision depend on relay row order, so the lookup refuses.
#[test]
fn test_multiple_heads_fail_the_lookup_rather_than_guessing() {
    let keys = nostr::Keys::generate();
    let events = [
        signed_persona(&keys, "test-persona", false),
        signed_persona(&keys, "test-persona", true),
    ];

    assert_eq!(
        head_state_from_events(&coordinate(), &events),
        HeadState::LookupFailed
    );
}

/// A row for a different coordinate cannot be used to answer this one. If it
/// were counted as a match, an unrelated event would mask a real absence; if
/// it were merely ignored without filtering, a mismatched relay response would
/// still read as "present".
#[test]
fn test_events_for_other_coordinates_are_ignored() {
    let keys = nostr::Keys::generate();
    let other = signed_persona(&keys, "a-different-persona", false);

    assert_eq!(
        head_state_from_events(&coordinate(), std::slice::from_ref(&other)),
        HeadState::Absent
    );
}

/// A kind mismatch is the cross-kind d-tag collision case: d-tags are not
/// unique across kinds, so a team with the same d-tag must not answer a
/// persona lookup.
#[test]
fn test_events_of_another_kind_do_not_answer_the_lookup() {
    let keys = nostr::Keys::generate();
    let persona = signed_persona(&keys, "test-persona", false);

    let team_coordinate = Coordinate {
        kind: KIND_TEAM,
        d_tag: "test-persona".to_string(),
    };
    assert_eq!(
        head_state_from_events(&team_coordinate, std::slice::from_ref(&persona)),
        HeadState::Absent
    );
}
