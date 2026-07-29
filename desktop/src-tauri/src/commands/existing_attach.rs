use tauri::{AppHandle, State};

use crate::{
    app_state::AppState,
    commands::agents::create_managed_agent,
    managed_agents::{
        build_attach_create_request, default_hermes_root, discover_acp_runtimes,
        find_candidate_by_id, list_existing_agent_candidates as discover_existing_agent_candidates,
        load_managed_agents, AcpAvailabilityStatus, AttachExistingAgentRequest,
        CreateManagedAgentResponse, ExistingAgentCandidate,
    },
};

fn hermes_runtime_is_available() -> bool {
    discover_acp_runtimes().into_iter().any(|runtime| {
        runtime.id == "hermes" && runtime.availability == AcpAvailabilityStatus::Available
    })
}

#[tauri::command]
pub async fn list_existing_agent_candidates(
    app: AppHandle,
) -> Result<Vec<ExistingAgentCandidate>, String> {
    let records = load_managed_agents(&app)?;
    let hermes_available = tokio::task::spawn_blocking(hermes_runtime_is_available)
        .await
        .map_err(|e| format!("spawn_blocking failed: {e}"))?;
    Ok(discover_existing_agent_candidates(
        default_hermes_root().as_deref(),
        &records,
        hermes_available,
    ))
}

#[tauri::command]
pub async fn attach_existing_agent(
    input: AttachExistingAgentRequest,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<CreateManagedAgentResponse, String> {
    let records = load_managed_agents(&app)?;
    let hermes_available = tokio::task::spawn_blocking(hermes_runtime_is_available)
        .await
        .map_err(|e| format!("spawn_blocking failed: {e}"))?;
    let candidates = discover_existing_agent_candidates(
        default_hermes_root().as_deref(),
        &records,
        hermes_available,
    );
    let candidate = find_candidate_by_id(&candidates, &input.candidate_id)?;
    let create_input = build_attach_create_request(
        candidate,
        input.name.as_deref(),
        input.spawn_after_attach,
        input.start_on_app_launch,
    )?;
    create_managed_agent(create_input, app, state).await
}
