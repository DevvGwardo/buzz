//! Discover and attach agents that already exist on the machine.
//!
//! v1 targets Hermes Agent homes: the default `~/.hermes` install plus
//! named profiles under `~/.hermes/profiles/<name>/`. Attach maps each
//! candidate onto a managed agent with Rocky defaults (owner-only,
//! parallelism 1, Buzz NIP-AE memory off so Hermes memory stays sole).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::managed_agents::types::{CreateManagedAgentRequest, RespondTo};
use crate::managed_agents::{normalize_agent_args, ManagedAgentRecord};

const HERMES_RUNTIME_ID: &str = "hermes";
const HERMES_COMMAND: &str = "hermes";
const DEFAULT_CANDIDATE_ID: &str = "hermes:default";
const BUZZ_ACP_NO_MEMORY: &str = "BUZZ_ACP_NO_MEMORY";
const HERMES_HOME_ENV: &str = "HERMES_HOME";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExistingAgentCandidate {
    /// Stable id: `hermes:default` or `hermes:profile:<name>`.
    pub id: String,
    pub runtime_id: String,
    pub display_name: String,
    /// Short path label for UI (`~/.hermes`, `profile:nub`).
    pub source_label: String,
    pub hermes_home: String,
    pub agent_command: String,
    pub agent_args: Vec<String>,
    pub already_attached: bool,
    pub attached_agent_pubkey: Option<String>,
    pub attachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachExistingAgentRequest {
    pub candidate_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub spawn_after_attach: bool,
    #[serde(default = "default_true")]
    pub start_on_app_launch: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone)]
struct HermesCandidateDraft {
    id: String,
    display_name: String,
    source_label: String,
    hermes_home: PathBuf,
    agent_args: Vec<String>,
}

/// List Hermes homes/profiles that can be attached as Buzz managed agents.
pub fn list_existing_agent_candidates(
    hermes_root: Option<&Path>,
    records: &[ManagedAgentRecord],
    hermes_runtime_available: bool,
) -> Vec<ExistingAgentCandidate> {
    let Some(root) = hermes_root
        .map(Path::to_path_buf)
        .or_else(default_hermes_root)
    else {
        return Vec::new();
    };

    let drafts = discover_hermes_drafts(&root);
    drafts
        .into_iter()
        .map(|draft| finalize_candidate(draft, records, hermes_runtime_available))
        .collect()
}

pub fn build_attach_create_request(
    candidate: &ExistingAgentCandidate,
    name_override: Option<&str>,
    spawn_after_attach: bool,
    start_on_app_launch: bool,
) -> Result<CreateManagedAgentRequest, String> {
    if !candidate.attachable {
        return Err(candidate
            .skip_reason
            .clone()
            .unwrap_or_else(|| format!("candidate {} is not attachable", candidate.id)));
    }
    if candidate.already_attached {
        return Err(format!(
            "candidate {} is already attached{}",
            candidate.id,
            candidate
                .attached_agent_pubkey
                .as_deref()
                .map(|pk| format!(" as {pk}"))
                .unwrap_or_default()
        ));
    }

    let name = name_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(candidate.display_name.as_str())
        .to_string();

    let mut env_vars = BTreeMap::new();
    env_vars.insert(HERMES_HOME_ENV.to_string(), candidate.hermes_home.clone());
    env_vars.insert(BUZZ_ACP_NO_MEMORY.to_string(), "true".to_string());

    Ok(CreateManagedAgentRequest {
        name,
        persona_id: None,
        team_id: None,
        relay_url: None,
        acp_command: None,
        agent_command: Some(candidate.agent_command.clone()),
        harness_override: true,
        agent_args: candidate.agent_args.clone(),
        mcp_command: None,
        turn_timeout_seconds: None,
        idle_timeout_seconds: None,
        max_turn_duration_seconds: None,
        parallelism: Some(1),
        system_prompt: Some(String::new()),
        avatar_url: None,
        model: None,
        provider: None,
        env_vars,
        spawn_after_create: spawn_after_attach,
        start_on_app_launch,
        backend: Default::default(),
        respond_to: Some(RespondTo::OwnerOnly),
        respond_to_allowlist: Vec::new(),
        relay_mesh: None,
    })
}

pub fn find_candidate_by_id<'a>(
    candidates: &'a [ExistingAgentCandidate],
    candidate_id: &str,
) -> Result<&'a ExistingAgentCandidate, String> {
    let id = candidate_id.trim();
    if id.is_empty() {
        return Err("candidate id is required".to_string());
    }
    candidates
        .iter()
        .find(|candidate| candidate.id == id)
        .ok_or_else(|| format!("unknown existing-agent candidate: {id}"))
}

pub fn default_hermes_root() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("HERMES_HOME") {
        let trimmed = root.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    dirs::home_dir().map(|home| home.join(".hermes"))
}

fn discover_hermes_drafts(root: &Path) -> Vec<HermesCandidateDraft> {
    let mut drafts = Vec::new();

    if root.join("config.yaml").is_file() {
        drafts.push(HermesCandidateDraft {
            id: DEFAULT_CANDIDATE_ID.to_string(),
            display_name: display_name_for_home(root, "Hermes"),
            source_label: "~/.hermes".to_string(),
            hermes_home: root.to_path_buf(),
            agent_args: Vec::new(),
        });
    }

    let profiles_dir = root.join("profiles");
    let Ok(entries) = fs::read_dir(&profiles_dir) else {
        return drafts;
    };

    let mut named = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        if !is_attachable_profile_name(&name) {
            continue;
        }
        if !path.join("config.yaml").is_file() {
            continue;
        }
        named.push(HermesCandidateDraft {
            id: format!("hermes:profile:{name}"),
            display_name: display_name_for_home(&path, &name),
            source_label: format!("profile:{name}"),
            hermes_home: path,
            agent_args: vec!["-p".to_string(), name, "acp".to_string()],
        });
    }
    named.sort_by(|left, right| left.id.cmp(&right.id));
    drafts.extend(named);
    drafts
}

fn finalize_candidate(
    draft: HermesCandidateDraft,
    records: &[ManagedAgentRecord],
    hermes_runtime_available: bool,
) -> ExistingAgentCandidate {
    let hermes_home = draft.hermes_home.to_string_lossy().into_owned();
    let (already_attached, attached_agent_pubkey) =
        find_attached_agent(records, &draft.hermes_home, &draft.agent_args);

    let (attachable, skip_reason) = if !hermes_runtime_available {
        (
            false,
            Some(
                "Hermes runtime is not available. Install Hermes Agent with ACP support first."
                    .to_string(),
            ),
        )
    } else if already_attached {
        (
            false,
            Some("Already attached as a Buzz managed agent.".to_string()),
        )
    } else {
        (true, None)
    };

    ExistingAgentCandidate {
        id: draft.id,
        runtime_id: HERMES_RUNTIME_ID.to_string(),
        display_name: draft.display_name,
        source_label: draft.source_label,
        hermes_home,
        agent_command: HERMES_COMMAND.to_string(),
        agent_args: normalize_agent_args(HERMES_COMMAND, draft.agent_args),
        already_attached,
        attached_agent_pubkey,
        attachable,
        skip_reason,
    }
}

fn find_attached_agent(
    records: &[ManagedAgentRecord],
    hermes_home: &Path,
    agent_args: &[String],
) -> (bool, Option<String>) {
    let target_home = canonicalize_lossy(hermes_home);
    for record in records {
        if !is_hermes_command(&record.agent_command) {
            continue;
        }

        let record_home = record
            .env_vars
            .get(HERMES_HOME_ENV)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| canonicalize_lossy(&path));

        if record_home
            .as_ref()
            .is_some_and(|home| home == &target_home)
        {
            return (true, Some(record.pubkey.clone()));
        }

        // Legacy attaches may only pin via `-p <name> acp` without HERMES_HOME.
        if record_home.is_none() && args_equivalent(&record.agent_args, agent_args) {
            return (true, Some(record.pubkey.clone()));
        }
    }
    (false, None)
}

fn is_hermes_command(command: &str) -> bool {
    let identity = Path::new(command.trim())
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase()
        .replace('_', "-");
    identity == "hermes" || identity == "hermes-acp"
}

fn args_equivalent(left: &[String], right: &[String]) -> bool {
    let normalized_left = normalize_agent_args(HERMES_COMMAND, left.to_vec());
    let normalized_right = normalize_agent_args(HERMES_COMMAND, right.to_vec());
    normalized_left == normalized_right
}

fn canonicalize_lossy(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn is_attachable_profile_name(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.starts_with('.') {
        return false;
    }
    // Hermes gateway / ephemeral session workspaces.
    if trimmed.starts_with("session-") {
        return false;
    }
    trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn display_name_for_home(home: &Path, fallback: &str) -> String {
    if let Some(from_soul) = read_soul_display_name(&home.join("SOUL.md")) {
        return from_soul;
    }
    title_case_slug(fallback)
}

fn read_soul_display_name(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    for line in raw.lines().take(40) {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            let mut name = rest.trim().to_string();
            for prefix in [
                "Hermes Agent Persona —",
                "Hermes Agent Persona -",
                "Hermes Agent Persona–",
                "Hermes Agent Persona—",
                "Hermes Agent Persona",
            ] {
                if let Some(stripped) = name.strip_prefix(prefix) {
                    name = stripped.trim().to_string();
                    break;
                }
            }
            name = name
                .trim_start_matches(['—', '–', '-', ':'])
                .trim()
                .to_string();
            // Prefer "nub (artist profile)" → "nub" when formatted that way,
            // otherwise keep a short heading.
            if let Some((before, _)) = name.split_once('(') {
                let cleaned = before.trim();
                if !cleaned.is_empty() {
                    return Some(title_case_slug(cleaned));
                }
            }
            if !name.is_empty() && name.len() <= 64 {
                return Some(if name.contains(' ') {
                    name
                } else {
                    title_case_slug(&name)
                });
            }
        }
        if let Some(rest) = trimmed.strip_prefix("You are ") {
            let token = rest
                .split(|c: char| c == '.' || c == ',' || c == '—' || c == '-')
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-');
            if !token.is_empty() && token.len() <= 48 {
                return Some(title_case_slug(token));
            }
        }
    }
    None
}

fn title_case_slug(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "Hermes".to_string();
    }
    if trimmed
        .chars()
        .any(|c| c.is_ascii_uppercase() && trimmed.contains(' '))
    {
        return trimmed.to_string();
    }
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => "Hermes".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managed_agents::types::ManagedAgentRecord;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("buzz-existing-attach-{label}-{nanos}"))
    }

    fn write_home(path: &Path, soul: &str) {
        fs::create_dir_all(path).expect("mkdir");
        fs::write(path.join("config.yaml"), "model:\n  default: test\n").expect("config");
        fs::write(path.join("SOUL.md"), soul).expect("soul");
    }

    fn blank_record(pubkey: &str, hermes_home: &str, args: Vec<&str>) -> ManagedAgentRecord {
        let mut record = ManagedAgentRecord {
            pubkey: pubkey.to_string(),
            name: "Attached".to_string(),
            persona_id: None,
            private_key_nsec: String::new(),
            auth_tag: None,
            relay_url: String::new(),
            avatar_url: None,
            acp_command: "buzz-acp".to_string(),
            agent_command: "hermes".to_string(),
            agent_command_override: Some("hermes".to_string()),
            agent_args: args.into_iter().map(str::to_string).collect(),
            mcp_command: String::new(),
            turn_timeout_seconds: 600,
            idle_timeout_seconds: None,
            max_turn_duration_seconds: None,
            parallelism: 1,
            system_prompt: None,
            model: None,
            provider: None,
            persona_source_version: None,
            env_vars: BTreeMap::new(),
            start_on_app_launch: true,
            auto_restart_on_config_change: true,
            runtime_pid: None,
            backend: Default::default(),
            backend_agent_id: None,
            provider_binary_path: None,
            team_id: None,
            persona_team_dir: None,
            persona_name_in_team: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            last_started_at: None,
            last_stopped_at: None,
            last_exit_code: None,
            last_error: None,
            last_error_code: None,
            respond_to: RespondTo::OwnerOnly,
            respond_to_allowlist: Vec::new(),
            display_name: None,
            slug: None,
            runtime: Some("hermes".to_string()),
            name_pool: Vec::new(),
            is_builtin: false,
            is_active: true,
            source_team: None,
            source_team_persona_slug: None,
            definition_respond_to: None,
            definition_respond_to_allowlist: Vec::new(),
            definition_parallelism: None,
            relay_mesh: None,
        };
        if !hermes_home.is_empty() {
            record
                .env_vars
                .insert(HERMES_HOME_ENV.to_string(), hermes_home.to_string());
        }
        record
    }

    #[test]
    fn discovers_default_and_named_profiles_skips_sessions() {
        let root = temp_root("discover");
        write_home(&root, "# Hermes Agent Persona — Rocky\n\nYou are Rocky.\n");
        write_home(
            &root.join("profiles").join("nub"),
            "# Hermes Agent Persona — nub (artist profile)\n\nYou are nub.\n",
        );
        write_home(
            &root.join("profiles").join("session-2604200308-1"),
            "# Temp\n",
        );
        // Profile without config.yaml must be ignored.
        fs::create_dir_all(root.join("profiles").join("empty")).expect("mkdir");

        let candidates = list_existing_agent_candidates(Some(&root), &[], true);
        let ids: Vec<_> = candidates.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["hermes:default", "hermes:profile:nub"]);
        assert_eq!(candidates[0].display_name, "Rocky");
        assert_eq!(candidates[1].display_name, "Nub");
        assert!(candidates[0].attachable);
        assert_eq!(candidates[0].agent_args, vec!["acp".to_string()]);
        assert_eq!(
            candidates[1].agent_args,
            vec!["-p".to_string(), "nub".to_string(), "acp".to_string()]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn marks_already_attached_by_hermes_home() {
        let root = temp_root("attached");
        write_home(&root, "# Hermes\n");
        let home = root.to_string_lossy().into_owned();
        let records = vec![blank_record("pk1", &home, vec!["acp"])];
        let candidates = list_existing_agent_candidates(Some(&root), &records, true);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].already_attached);
        assert!(!candidates[0].attachable);
        assert_eq!(
            candidates[0].attached_agent_pubkey.as_deref(),
            Some("pk1")
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn build_attach_request_applies_rocky_defaults() {
        let candidate = ExistingAgentCandidate {
            id: "hermes:default".to_string(),
            runtime_id: "hermes".to_string(),
            display_name: "Rocky".to_string(),
            source_label: "~/.hermes".to_string(),
            hermes_home: "/tmp/fake-hermes".to_string(),
            agent_command: "hermes".to_string(),
            agent_args: vec!["acp".to_string()],
            already_attached: false,
            attached_agent_pubkey: None,
            attachable: true,
            skip_reason: None,
        };
        let request = build_attach_create_request(&candidate, None, true, true).expect("ok");
        assert_eq!(request.name, "Rocky");
        assert_eq!(request.agent_command.as_deref(), Some("hermes"));
        assert!(request.harness_override);
        assert_eq!(request.parallelism, Some(1));
        assert_eq!(request.respond_to, Some(RespondTo::OwnerOnly));
        assert_eq!(
            request.env_vars.get("HERMES_HOME").map(String::as_str),
            Some("/tmp/fake-hermes")
        );
        assert_eq!(
            request.env_vars.get("BUZZ_ACP_NO_MEMORY").map(String::as_str),
            Some("true")
        );
        assert!(request.spawn_after_create);
    }

    #[test]
    fn build_attach_request_rejects_already_attached() {
        let candidate = ExistingAgentCandidate {
            id: "hermes:profile:nub".to_string(),
            runtime_id: "hermes".to_string(),
            display_name: "Nub".to_string(),
            source_label: "profile:nub".to_string(),
            hermes_home: "/tmp/nub".to_string(),
            agent_command: "hermes".to_string(),
            agent_args: vec!["-p".to_string(), "nub".to_string(), "acp".to_string()],
            already_attached: true,
            attached_agent_pubkey: Some("pk".to_string()),
            attachable: false,
            skip_reason: Some("Already attached".to_string()),
        };
        let err = build_attach_create_request(&candidate, None, false, true).unwrap_err();
        assert!(
            err.to_lowercase().contains("already attached"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn unavailable_runtime_is_not_attachable() {
        let root = temp_root("no-runtime");
        write_home(&root, "# Hermes\n");
        let candidates = list_existing_agent_candidates(Some(&root), &[], false);
        assert_eq!(candidates.len(), 1);
        assert!(!candidates[0].attachable);
        assert!(candidates[0]
            .skip_reason
            .as_deref()
            .unwrap_or("")
            .contains("not available"));
        let _ = fs::remove_dir_all(&root);
    }
}
