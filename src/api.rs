use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::process::Child;
use tracing::{error, info, warn};

use crate::{
    cleanup::{self, CleanupStrategy},
    exit_codes::ExitReason,
    provisioner, session_config,
    state::{AppState, SessionEntry, SessionState},
    supervisor,
};

/// Notification to backend about session status
async fn notify_backend(
    api_url: &str,
    api_key: &str,
    session_id: &str,
    status: &str,
    error: Option<&str>,
) {
    let payload = serde_json::json!({
        "sessionId": session_id,
        "status": status,
        "error": error,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });

    let client = reqwest::Client::new();
    let endpoint = format!("{}/api/internal/sessions/update", api_url);

    match client
        .post(&endpoint)
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&payload)
        .send()
        .await
    {
        Ok(response) => {
            if !response.status().is_success() {
                warn!(
                    session_id = %session_id,
                    status = %status,
                    http_status = %response.status(),
                    "Backend notification returned non-success status"
                );
            }
        }
        Err(e) => {
            warn!(
                session_id = %session_id,
                status = %status,
                error = %e,
                "Failed to notify backend"
            );
        }
    }
}

/// Background task that watches supervisor process and handles its exit
async fn watch_supervisor(
    session_id: String,
    mut supervisor: Child,
    app_state: AppState,
    cleanup_policy: session_config::CleanupConfig,
    backend_url: String,
    backend_key: String,
) {
    info!(
        session_id = %session_id,
        "Supervisor watcher started"
    );

    // Wait for supervisor to exit
    let exit_status = match supervisor.wait().await {
        Ok(status) => status,
        Err(e) => {
            error!(
                session_id = %session_id,
                error = %e,
                "Failed to wait for supervisor process"
            );

            // Update session state to failed
            {
                let mut sessions = app_state.sessions.lock().unwrap();
                if let Some(entry) = sessions.get_mut(&session_id) {
                    entry.state = SessionState::Failed(format!("Supervisor wait failed: {}", e));
                    entry.supervisor_handle = None;
                }
            }

            notify_backend(&backend_url, &backend_key, &session_id, "failed", Some("Supervisor process error")).await;
            return;
        }
    };

    let exit_code = exit_status.code().unwrap_or(100);

    info!(
        session_id = %session_id,
        exit_code = exit_code,
        "Supervisor exited"
    );

    // Get session metadata
    let (game_id, build_id) = {
        let sessions = app_state.sessions.lock().unwrap();
        match sessions.get(&session_id) {
            Some(entry) => (entry.game_id.clone(), entry.build_id.clone()),
            None => {
                error!(
                    session_id = %session_id,
                    "Session not found in state when supervisor exited"
                );
                return;
            }
        }
    };

    // Translate exit code to ExitReason
    let reason = ExitReason::from_exit_code(exit_code);

    info!(
        session_id = %session_id,
        exit_reason = %reason,
        "Translated supervisor exit code to reason"
    );

    // Decide if cleanup is needed based on policy
    let should_cleanup = match reason {
        ExitReason::GameExitedNormally => cleanup_policy.on_normal_exit,
        ExitReason::MaxDurationExceeded => cleanup_policy.on_timeout,
        _ if reason.is_violation() => cleanup_policy.on_violation,
        _ => false,
    };

    if should_cleanup {
        info!(
            session_id = %session_id,
            game_id = %game_id,
            build_id = %build_id,
            "Cleanup required based on policy"
        );

        // Determine cleanup strategy
        let strategy = if !cleanup_policy.delete_game_files {
            CleanupStrategy::DeleteSessionOnly
        } else if cleanup_policy.shared_build {
            // TODO: Check if other sessions are using this build
            CleanupStrategy::DeleteSessionOnly
        } else {
            CleanupStrategy::DeleteBuild
        };

        info!(
            session_id = %session_id,
            strategy = ?strategy,
            "Cleanup strategy determined"
        );

        // Update state to cleaning up
        {
            let mut sessions = app_state.sessions.lock().unwrap();
            if let Some(entry) = sessions.get_mut(&session_id) {
                entry.state = SessionState::CleaningUp;
            }
        }

        // Execute cleanup
        match cleanup::cleanup_session(&game_id, &build_id, strategy) {
            Ok(_) => {
                info!(
                    session_id = %session_id,
                    game_id = %game_id,
                    build_id = %build_id,
                    "Cleanup completed successfully"
                );
            }
            Err(e) => {
                error!(
                    session_id = %session_id,
                    error = %e,
                    "Cleanup failed"
                );
            }
        }
    } else {
        info!(
            session_id = %session_id,
            "No cleanup required based on policy"
        );
    }

    // Notify backend of session end
    notify_backend(&backend_url, &backend_key, &session_id, "ended", None).await;

    // Update final session state
    {
        let mut sessions = app_state.sessions.lock().unwrap();
        if let Some(entry) = sessions.get_mut(&session_id) {
            entry.state = SessionState::Ended;
            entry.supervisor_handle = None;
        }
    }

    info!(
        session_id = %session_id,
        "Supervisor watcher completed"
    );
}

#[derive(Deserialize)]
pub struct StartSessionRequest {
    pub session_id: String,
    pub game_id: String,
    pub build_id: String,
    pub s3_url: String,
    pub format: String,
    pub start_path: String,
    pub max_duration_seconds: u64,
    pub backend_api_url: String,
    pub backend_api_key: String,
    
    // Cleanup policy from backend
    #[serde(default)]
    pub cleanup_on_normal_exit: bool,
    #[serde(default = "default_cleanup_on_violation")]
    pub cleanup_on_violation: bool,
    #[serde(default = "default_cleanup_on_timeout")]
    pub cleanup_on_timeout: bool,
    #[serde(default)]
    pub delete_game_files: bool,
    #[serde(default)]
    pub shared_build: bool,
}

fn default_cleanup_on_violation() -> bool {
    true
}
fn default_cleanup_on_timeout() -> bool {
    true
}

pub async fn start_session(
    State(state): State<AppState>,
    Json(req): Json<StartSessionRequest>,
) -> Result<Json<serde_json::Value>, String> {
    info!(
        session_id = %req.session_id,
        game_id = %req.game_id,
        build_id = %req.build_id,
        "Starting session"
    );

    // Check for duplicate session
    {
        let sessions = state.sessions.lock().unwrap();
        if sessions.contains_key(&req.session_id) {
            return Err(format!("Session {} already exists", req.session_id));
        }
    }

    let build_root = format!("C:\\games\\{}\\{}", req.game_id, req.build_id);
    let exe_path = format!("{}\\game\\{}", build_root, req.start_path);
    let session_dir = Path::new(&build_root).join("session");

    // Create session entry in Provisioning state
    {
        state.sessions.lock().unwrap().insert(
            req.session_id.clone(),
            SessionEntry {
                state: SessionState::Provisioning,
                supervisor_handle: None,
                game_id: req.game_id.clone(),
                build_id: req.build_id.clone(),
                created_at: std::time::Instant::now(),
            },
        );
    }

    // Notify backend: provisioning started
    notify_backend(
        &req.backend_api_url,
        &req.backend_api_key,
        &req.session_id,
        "provisioning",
        None,
    )
    .await;

    // Provision game build
    if let Err(e) = provisioner::ensure_game_ready(
        &req.game_id,
        &req.build_id,
        &req.s3_url,
        &req.format,
    ) {
        error!(
            session_id = %req.session_id,
            error = %e,
            "Game provisioning failed"
        );

        notify_backend(
            &req.backend_api_url,
            &req.backend_api_key,
            &req.session_id,
            "failed",
            Some(&format!("Provisioning failed: {}", e)),
        )
        .await;

        // Update state to failed
        {
            let mut sessions = state.sessions.lock().unwrap();
            if let Some(entry) = sessions.get_mut(&req.session_id) {
                entry.state = SessionState::Failed(e.to_string());
            }
        }

        return Err(e.to_string());
    }

    // Build cleanup policy from request
    let cleanup_policy = session_config::CleanupConfig {
        on_normal_exit: req.cleanup_on_normal_exit,
        on_violation: req.cleanup_on_violation,
        on_timeout: req.cleanup_on_timeout,
        delete_game_files: req.delete_game_files,
        shared_build: req.shared_build,
    };

    // Write session.json with cleanup policy
    let session_json = match session_config::write_session_config(
        &session_dir,
        &req.session_id,
        &req.game_id,
        Path::new(&exe_path),
        req.max_duration_seconds,
        &req.backend_api_url,
        &req.backend_api_key,
        cleanup_policy.clone(),
    ) {
        Ok(p) => p,
        Err(e) => {
            error!(
                session_id = %req.session_id,
                error = %e,
                "Failed to write session config"
            );

            notify_backend(
                &req.backend_api_url,
                &req.backend_api_key,
                &req.session_id,
                "failed",
                Some(&format!("Session config error: {}", e)),
            )
            .await;

            // Update state to failed
            {
                let mut sessions = state.sessions.lock().unwrap();
                if let Some(entry) = sessions.get_mut(&req.session_id) {
                    entry.state = SessionState::Failed(e.to_string());
                }
            }

            return Err(e.to_string());
        }
    };

    // Notify backend: launching supervisor
    notify_backend(
        &req.backend_api_url,
        &req.backend_api_key,
        &req.session_id,
        "launching",
        None,
    )
    .await;

    // Spawn supervisor process
    let supervisor_child = match supervisor::start_supervisor(&session_json).await {
        Ok(c) => c,
        Err(e) => {
            error!(
                session_id = %req.session_id,
                error = %e,
                "Failed to start supervisor"
            );

            notify_backend(
                &req.backend_api_url,
                &req.backend_api_key,
                &req.session_id,
                "failed",
                Some(&format!("Supervisor failed to start: {}", e)),
            )
            .await;

            // Update state to failed
            {
                let mut sessions = state.sessions.lock().unwrap();
                if let Some(entry) = sessions.get_mut(&req.session_id) {
                    entry.state = SessionState::Failed(e.to_string());
                }
            }

            return Err(e.to_string());
        }
    };

    // Update state to Launching (supervisor spawned)
    {
        let mut sessions = state.sessions.lock().unwrap();
        if let Some(entry) = sessions.get_mut(&req.session_id) {
            entry.state = SessionState::Launching;
            entry.supervisor_handle = Some(supervisor_child.id().unwrap());
        }
    }

    // Spawn background watcher task
    let state_clone = state.clone();
    let session_id_clone = req.session_id.clone();
    let backend_url_clone = req.backend_api_url.clone();
    let backend_key_clone = req.backend_api_key.clone();

    tokio::task::spawn(async move {
        watch_supervisor(
            session_id_clone,
            supervisor_child,
            state_clone,
            cleanup_policy,
            backend_url_clone,
            backend_key_clone,
        )
        .await;
    });

    info!(
        session_id = %req.session_id,
        "Session started, supervisor watcher spawned"
    );

    Ok(Json(serde_json::json!({
        "status": "started",
        "session_id": req.session_id,
        "message": "Supervisor launched, session monitoring in progress"
    })))
}

#[derive(Deserialize)]
pub struct StopSessionRequest {
    pub session_id: String,
}

pub async fn stop_session(
    State(state): State<AppState>,
    Json(req): Json<StopSessionRequest>,
) -> Result<Json<serde_json::Value>, String> {
    info!(
        session_id = %req.session_id,
        "Stop session requested"
    );

    let supervisor_pid = {
        let mut sessions = state.sessions.lock().unwrap();
        let entry = sessions
            .get_mut(&req.session_id)
            .ok_or_else(|| format!("Session {} not found", req.session_id))?;

        if entry.supervisor_handle.is_none() {
            return Err("Supervisor not running".to_string());
        }

        let pid = entry.supervisor_handle.unwrap();

        // Mark as user-initiated stop
        entry.state = SessionState::Stopping;

        pid
    };

    // Kill supervisor process
    // The background watcher will detect the exit and handle cleanup
    match supervisor::kill_supervisor(supervisor_pid).await {
        Ok(_) => {
            info!(
                session_id = %req.session_id,
                supervisor_pid = supervisor_pid,
                "Supervisor process killed, watcher will handle cleanup"
            );

            Ok(Json(serde_json::json!({
                "status": "stopping",
                "session_id": req.session_id,
                "message": "Supervisor terminated, cleanup will be handled automatically"
            })))
        }
        Err(e) => {
            error!(
                session_id = %req.session_id,
                error = %e,
                "Failed to kill supervisor"
            );

            Err(format!("Failed to stop supervisor: {}", e))
        }
    }
}

#[derive(Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub state: String,
    pub game_id: String,
    pub build_id: String,
    pub uptime_seconds: u64,
}

pub async fn list_sessions(State(state): State<AppState>) -> Json<Vec<SessionInfo>> {
    let sessions = state.sessions.lock().unwrap();

    let list: Vec<SessionInfo> = sessions
        .iter()
        .map(|(id, entry)| SessionInfo {
            session_id: id.clone(),
            state: format!("{:?}", entry.state),
            game_id: entry.game_id.clone(),
            build_id: entry.build_id.clone(),
            uptime_seconds: entry.created_at.elapsed().as_secs(),
        })
        .collect();

    Json(list)
}