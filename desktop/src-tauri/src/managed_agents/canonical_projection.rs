//! The canonical published projection of a config record (V3.1).
//!
//! Every comparison the boot decision pass makes — disk against baseline, disk
//! against the relay head, baseline against what a publish would emit — runs on
//! this projection, never on whole JSON and never on content alone.
//!
//! Two reasons it cannot be either of those:
//!
//! - **Whole JSON is too wide.** Records carry install-local fields the relay
//!   never sees (`source_dir`, `is_symlink`, `version`, timestamps, and for
//!   managed agents `private_key_nsec`, `auth_tag`, `env_vars`). Comparing them
//!   would report a difference for two records that publish identically, and
//!   the pass would "repair" a divergence that does not exist.
//! - **Content alone is too narrow.** A persona's `shared` state rides in a
//!   relay-significant *tag*, not in the content body. Two personas with equal
//!   content but different `shared` tags publish as different events, so a
//!   content-only compare would call them equal and skip a real publish.
//!
//! The projection is therefore exactly what publishing emits: the event content
//! plus the relay-significant tags, in a form that compares by value.

use buzz_core_pkg::kind::{KIND_MANAGED_AGENT, KIND_PERSONA, KIND_TEAM};

use super::{
    agent_events::agent_event_content, persona_events::persona_event_content,
    team_events::team_event_content, AgentDefinition, ManagedAgentRecord, TeamRecord,
};

/// What one coordinate publishes: the event content and the relay-significant
/// tags that travel with it.
///
/// Equality is the decision-table's notion of "same": two projections are equal
/// exactly when publishing both would put the same thing on the relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalProjection {
    /// The serialized event content, byte-identical to what the builder emits.
    pub content: String,
    /// Whether the persona carries `["shared", "true"]`. Always `false` for
    /// teams and managed agents, whose builders emit no `shared` tag.
    pub shared: bool,
}

impl CanonicalProjection {
    /// The projection a persona (kind:30175) would publish.
    ///
    /// `shared` is read from the record rather than inferred, because it is the
    /// field the builder turns into the tag.
    pub fn for_persona(record: &AgentDefinition) -> Result<Self, String> {
        Ok(Self {
            content: serde_json::to_string(&persona_event_content(record))
                .map_err(|e| format!("failed to serialize persona projection: {e}"))?,
            shared: record.shared,
        })
    }

    /// The projection a team (kind:30176) would publish.
    pub fn for_team(record: &TeamRecord) -> Result<Self, String> {
        Ok(Self {
            content: serde_json::to_string(&team_event_content(record))
                .map_err(|e| format!("failed to serialize team projection: {e}"))?,
            shared: false,
        })
    }

    /// The projection a managed agent (kind:30177) would publish.
    ///
    /// `agent_event_content` is the opt-IN no-secrets allowlist, so this can
    /// never carry a private key, auth tag, or env var.
    pub fn for_managed_agent(record: &ManagedAgentRecord) -> Result<Self, String> {
        Ok(Self {
            content: serde_json::to_string(&agent_event_content(record))
                .map_err(|e| format!("failed to serialize managed-agent projection: {e}"))?,
            shared: false,
        })
    }

    /// The projection an already-signed event carries.
    ///
    /// This is the head side of a disk-vs-head compare. `shared` is read with
    /// the same fail-closed helper the relay's read-authorization uses, so a
    /// malformed tag shape is treated as not-shared on both sides of the
    /// comparison rather than only one.
    pub fn from_event(event: &nostr::Event) -> Self {
        let kind = event.kind.as_u16() as u32;
        Self {
            content: event.content.to_string(),
            shared: kind == KIND_PERSONA && buzz_core_pkg::kind::event_is_shared(event),
        }
    }

    /// Whether `kind` is a config kind this projection knows how to build.
    pub fn is_config_kind(kind: u32) -> bool {
        matches!(kind, KIND_PERSONA | KIND_TEAM | KIND_MANAGED_AGENT)
    }
}

#[cfg(test)]
mod tests;
