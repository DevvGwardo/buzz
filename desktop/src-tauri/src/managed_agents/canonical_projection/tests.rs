use std::collections::BTreeMap;
use std::path::PathBuf;

use super::*;
use crate::managed_agents::persona_events::build_persona_event;

fn sample_persona() -> AgentDefinition {
    AgentDefinition {
        id: "test-persona".to_string(),
        display_name: "Test Persona".to_string(),
        avatar_url: Some("https://example.com/avatar.png".to_string()),
        system_prompt: "You are a test assistant.".to_string(),
        runtime: Some("goose".to_string()),
        model: Some("claude-opus-4".to_string()),
        provider: Some("anthropic".to_string()),
        name_pool: vec!["Alpha".to_string()],
        is_builtin: false,
        is_active: true,
        shared: false,
        source_team: None,
        source_team_persona_slug: Some("test-slug".to_string()),
        catalog_source: None,
        env_vars: BTreeMap::from([("KEY".to_string(), "value".to_string())]),
        respond_to: None,
        respond_to_allowlist: Vec::new(),
        parallelism: None,
        created_at: "2025-01-01T00:00:00Z".to_string(),
        updated_at: "2025-01-01T00:00:00Z".to_string(),
    }
}

fn sample_team() -> TeamRecord {
    TeamRecord {
        id: "team-123".to_string(),
        name: "Test Team".to_string(),
        description: Some("A test team".to_string()),
        instructions: Some("Coordinate carefully.".to_string()),
        persona_ids: vec!["p1".to_string()],
        is_builtin: false,
        source_dir: Some(PathBuf::from("/local/only/path")),
        is_symlink: true,
        symlink_target: Some("/somewhere".to_string()),
        version: Some("1.0".to_string()),
        created_at: "2025-01-01T00:00:00Z".to_string(),
        updated_at: "2025-01-01T00:00:00Z".to_string(),
    }
}

/// The whole point of the projection: a field the relay never sees must not
/// register as a difference, or the decision pass "repairs" a divergence that
/// does not exist and republishes on every boot.
#[test]
fn local_only_persona_fields_do_not_change_the_projection() {
    let base = sample_persona();
    let local_edit = AgentDefinition {
        // None of these are in the published content.
        is_active: !base.is_active,
        source_team: Some("some-team".to_string()),
        env_vars: BTreeMap::from([("OTHER".to_string(), "different".to_string())]),
        created_at: "2099-01-01T00:00:00Z".to_string(),
        updated_at: "2099-01-01T00:00:00Z".to_string(),
        ..base.clone()
    };

    assert_eq!(
        CanonicalProjection::for_persona(&base).unwrap(),
        CanonicalProjection::for_persona(&local_edit).unwrap()
    );
}

/// A published field must register, or a real edit is silently never published.
#[test]
fn published_persona_fields_change_the_projection() {
    let base = sample_persona();
    for edited in [
        AgentDefinition {
            system_prompt: "Different prompt.".to_string(),
            ..base.clone()
        },
        AgentDefinition {
            model: Some("other-model".to_string()),
            ..base.clone()
        },
        AgentDefinition {
            provider: Some("other-provider".to_string()),
            ..base.clone()
        },
        AgentDefinition {
            display_name: "Renamed".to_string(),
            ..base.clone()
        },
    ] {
        assert_ne!(
            CanonicalProjection::for_persona(&base).unwrap(),
            CanonicalProjection::for_persona(&edited).unwrap(),
            "a published field edit must be visible to the decision pass"
        );
    }
}

/// `shared` rides in a relay-significant TAG, not the content body. A
/// content-only compare would call these two equal and skip a real publish —
/// this is the case that makes "content alone" wrong.
#[test]
fn shared_tag_difference_is_visible_even_when_content_matches() {
    let private = sample_persona();
    let shared = AgentDefinition {
        shared: true,
        ..private.clone()
    };

    let private_projection = CanonicalProjection::for_persona(&private).unwrap();
    let shared_projection = CanonicalProjection::for_persona(&shared).unwrap();

    assert_eq!(
        private_projection.content, shared_projection.content,
        "precondition: the content bodies are identical"
    );
    assert_ne!(
        private_projection, shared_projection,
        "the shared tag must still make the projections differ"
    );
}

/// Disk-vs-head is only meaningful if both sides project identically for a
/// record that was published unchanged.
#[test]
fn disk_projection_round_trips_through_a_signed_event() {
    let keys = nostr::Keys::generate();
    for shared in [false, true] {
        let record = AgentDefinition {
            shared,
            ..sample_persona()
        };
        let event = build_persona_event(&record)
            .unwrap()
            .sign_with_keys(&keys)
            .unwrap();

        assert_eq!(
            CanonicalProjection::for_persona(&record).unwrap(),
            CanonicalProjection::from_event(&event),
            "a freshly published record must project equal to its own event"
        );
    }
}

/// Teams and managed agents emit no `shared` tag, so their projections must
/// never claim one — otherwise a team would compare unequal to its own event.
#[test]
fn non_persona_kinds_are_never_shared() {
    assert!(
        !CanonicalProjection::for_team(&sample_team())
            .unwrap()
            .shared
    );

    let keys = nostr::Keys::generate();
    let event = crate::managed_agents::team_events::build_team_event(&sample_team())
        .unwrap()
        .sign_with_keys(&keys)
        .unwrap();
    assert_eq!(
        CanonicalProjection::for_team(&sample_team()).unwrap(),
        CanonicalProjection::from_event(&event)
    );
}

/// A team's install-local fields (`source_dir`, `is_symlink`, `version`) are
/// exactly the ones V3.10 cites as unrecoverable from a relay projection, so
/// they must not drive a publish decision either.
#[test]
fn local_only_team_fields_do_not_change_the_projection() {
    let base = sample_team();
    let local_edit = TeamRecord {
        source_dir: Some(PathBuf::from("/a/completely/different/path")),
        is_symlink: !base.is_symlink,
        symlink_target: None,
        version: Some("9.9".to_string()),
        updated_at: "2099-01-01T00:00:00Z".to_string(),
        ..base.clone()
    };

    assert_eq!(
        CanonicalProjection::for_team(&base).unwrap(),
        CanonicalProjection::for_team(&local_edit).unwrap()
    );
}

#[test]
fn config_kinds_are_recognized_and_others_are_not() {
    for kind in [KIND_PERSONA, KIND_TEAM, KIND_MANAGED_AGENT] {
        assert!(CanonicalProjection::is_config_kind(kind));
    }
    // kind:5 deletions and chat messages are not config coordinates.
    for kind in [1u32, 5, 30178] {
        assert!(!CanonicalProjection::is_config_kind(kind));
    }
}
