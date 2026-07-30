use std::collections::BTreeMap;
use std::path::PathBuf;

use super::*;
use crate::managed_agents::{AgentDefinition, ManagedAgentRecord, TeamRecord};

fn persona(id: &str, slug: Option<&str>, builtin: bool) -> AgentDefinition {
    AgentDefinition {
        id: id.to_string(),
        display_name: "Test".to_string(),
        avatar_url: None,
        system_prompt: "prompt".to_string(),
        runtime: None,
        model: None,
        provider: None,
        name_pool: Vec::new(),
        is_builtin: builtin,
        is_active: true,
        shared: false,
        source_team: None,
        source_team_persona_slug: slug.map(str::to_string),
        catalog_source: None,
        env_vars: BTreeMap::new(),
        respond_to: None,
        respond_to_allowlist: Vec::new(),
        parallelism: None,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: "2026-01-01T00:00:00Z".to_string(),
    }
}

fn team(id: &str, builtin: bool) -> TeamRecord {
    TeamRecord {
        id: id.to_string(),
        name: "Team".to_string(),
        description: None,
        instructions: None,
        persona_ids: Vec::new(),
        is_builtin: builtin,
        source_dir: Some(PathBuf::from("/local/only")),
        is_symlink: false,
        symlink_target: None,
        version: None,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: "2026-01-01T00:00:00Z".to_string(),
    }
}

fn agent(pubkey: &str) -> ManagedAgentRecord {
    serde_json::from_str(&format!(
        r#"{{
            "pubkey": "{pubkey}",
            "name": "Agent",
            "relay_url": "wss://localhost:3000",
            "acp_command": "buzz-acp",
            "agent_command": "goose",
            "agent_args": [],
            "mcp_command": "",
            "turn_timeout_seconds": 320,
            "system_prompt": "You are a test agent.",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "last_started_at": null,
            "last_stopped_at": null,
            "last_exit_code": null,
            "last_error": null
        }}"#
    ))
    .unwrap()
}

fn coordinates(records: &[(Coordinate, Option<CanonicalProjection>)]) -> Vec<(u32, String)> {
    records
        .iter()
        .map(|(coordinate, _)| (coordinate.kind, coordinate.d_tag.clone()))
        .collect()
}

/// All three config kinds must be enumerated. A kind the barrier does not
/// enumerate is a kind it can never suppress, so the revert bug would survive
/// for exactly those records.
#[test]
fn test_all_three_config_kinds_are_enumerated() {
    let projected = project_records(
        &[persona("p1", None, false)],
        &[team("t1", false)],
        &[agent("agentpubkey")],
    );

    assert_eq!(
        coordinates(&projected),
        vec![
            (KIND_PERSONA, "p1".to_string()),
            (KIND_TEAM, "t1".to_string()),
            (KIND_MANAGED_AGENT, "agentpubkey".to_string()),
        ]
    );
}

/// Built-ins ship in the binary and are never published, so a head lookup for
/// one would always report `Absent` and gate a coordinate that has nothing to
/// gate.
#[test]
fn test_builtin_records_are_not_enumerated() {
    let projected = project_records(
        &[persona("builtin", None, true), persona("real", None, false)],
        &[team("builtin-team", true), team("real-team", false)],
        &[],
    );

    assert_eq!(
        coordinates(&projected),
        vec![
            (KIND_PERSONA, "real".to_string()),
            (KIND_TEAM, "real-team".to_string()),
        ]
    );
}

/// A key-less agent has no coordinate at all — its d-tag IS its pubkey, minted
/// on first start. Enumerating it would look up `{"#d": [""]}`.
#[test]
fn test_keyless_agents_are_not_enumerated() {
    let projected = project_records(&[], &[], &[agent(""), agent("haskey")]);

    assert_eq!(
        coordinates(&projected),
        vec![(KIND_MANAGED_AGENT, "haskey".to_string())]
    );
}

/// The enumerated d-tag must be the one the publish path would use, or the
/// barrier looks up a head for a coordinate this device never writes to and
/// reads a spurious absence. `persona_d_tag` prefers the team slug and
/// normalizes it.
#[test]
fn test_persona_coordinate_uses_the_published_d_tag_derivation() {
    let projected = project_records(&[persona("uuid-id", Some("CodeReviewer"), false)], &[], &[]);

    assert_eq!(
        coordinates(&projected),
        vec![(KIND_PERSONA, "codereviewer".to_string())]
    );
}

/// Two personas can normalize onto one d-tag. Deciding it twice would gate the
/// same coordinate from the same inputs twice; the second pass is noise.
#[test]
fn test_duplicate_coordinates_are_decided_once() {
    let projected = project_records(
        &[
            persona("first", Some("Shared"), false),
            persona("second", Some("shared"), false),
        ],
        &[],
        &[],
    );

    assert_eq!(
        coordinates(&projected),
        vec![(KIND_PERSONA, "shared".to_string())]
    );
}

/// A d-tag collision ACROSS kinds is not a duplicate: `30175:x` and `30176:x`
/// are different coordinates with different heads, and collapsing them would
/// let a team's head answer a persona's lookup.
#[test]
fn test_same_d_tag_on_different_kinds_are_separate_coordinates() {
    let projected = project_records(&[persona("same", None, false)], &[team("same", false)], &[]);

    assert_eq!(
        coordinates(&projected),
        vec![
            (KIND_PERSONA, "same".to_string()),
            (KIND_TEAM, "same".to_string()),
        ]
    );
}

/// The projection carried into the decision must be what publishing emits, or
/// every disk-vs-head compare is against the wrong value.
#[test]
fn test_projection_matches_what_publishing_would_emit() {
    let record = persona("p1", None, false);
    let projected = project_records(std::slice::from_ref(&record), &[], &[]);

    assert_eq!(
        projected[0].1,
        Some(CanonicalProjection::for_persona(&record).unwrap())
    );
}

/// Empty stores are the fresh-install case: no coordinates, no lookups, no
/// gate writes.
#[test]
fn test_no_records_yields_no_coordinates() {
    assert!(project_records(&[], &[], &[]).is_empty());
}
