use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::process::Child;
use tracing::{error, info, warn};
use std::process::Stdio;
use tokio::process::Command;
use tokio::net::TcpStream;
use std::time::Duration;

use crate::{
    cleanup::{self, CleanupStrategy},
    exit_codes::ExitReason,
    provisioner, session_config,
    state::{AppState, SessionEntry, SessionState},
    supervisor,
};

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

// ✨ NEW: Verify cleanup actually happened
async fn verify_cleanup_complete(game_id: &str, build_id: &str) -> bool {
    let build_root = format!("C:\\games\\{}\\{}", game_id, build_id);
    let session_dir = Path::new(&build_root).join("session");
    
    if session_dir.exists() {
        warn!(
            game_id = %game_id,
            build_id = %build_id,
            "Session directory still exists after cleanup"
        );
        return false;
    }
    
    true
}

// ✨ NEW: Force cleanup if verification fails
async fn force_cleanup(game_id: &str, build_id: &str) -> Result<(), String> {
    use std::process::Command;
    
    let build_root = format!("C:\\games\\{}\\{}", game_id, build_id);
    
    info!(
        game_id = %game_id,
        build_id = %build_id,
        "Attempting force cleanup"
    );
    
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Remove-Item -Path '{}\\session' -Recurse -Force -ErrorAction SilentlyContinue; exit $?",
                build_root
            ),
        ])
        .output()
        .map_err(|e| format!("Failed to execute cleanup: {}", e))?;

    if output.status.success() {
        info!("Force cleanup succeeded");
        Ok(())
    } else {
        Err(format!(
            "Force cleanup failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

// ✨ NEW: Clean session from memory
async fn cleanup_session_resources(
    session_id: &str,
    app_state: &AppState,
) {
    info!(
        session_id = %session_id,
        "Cleaning up session resources"
    );
    
    let config_cache = format!("C:\\Instance\\cache\\{}.json", session_id);
    let _ = std::fs::remove_file(&config_cache);
    
    info!(
        session_id = %session_id,
        "Session resources cleaned up"
    );
}

// ✨ NEW: Gracefully stop web server
async fn stop_web_server_graceful() -> Result<(), String> {
    info!("Attempting graceful web server shutdown...");
    
    let output = std::process::Command::new("taskkill")
        .args(["/IM", "web-server.exe"])
        .output()
        .map_err(|e| format!("Failed to execute taskkill: {}", e))?;

    if output.status.success() {
        info!("Graceful shutdown initiated");
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        Ok(())
    } else {
        warn!("Graceful shutdown failed, will attempt force kill");
        Err("Graceful shutdown failed".to_string())
    }
}

// ✨ NEW: Force kill web server
async fn force_kill_web_server() -> Result<(), String> {
    info!("Force killing web server...");
    
    let output = std::process::Command::new("taskkill")
        .args(["/F", "/IM", "web-server.exe"])
        .output()
        .map_err(|e| format!("Failed to execute taskkill: {}", e))?;

    if output.status.success() {
        info!("Web server force killed");
        Ok(())
    } else {
        warn!("Force kill output: {}", String::from_utf8_lossy(&output.stderr));
        Ok(())
    }
}

// ✨ NEW: Start web server
async fn start_web_server() -> Result<u32, String> {
    info!("Starting web server...");
    
    let child = std::process::Command::new("C:\\Instance\\web-server.exe")
        .spawn()
        .map_err(|e| format!("Failed to start web server: {}", e))?;

    let pid = child.id();
    info!(web_server_pid = pid, "Web server started successfully");
    
    Ok(pid)
}

// ✨ NEW: Restart web server (graceful → force → restart)
async fn restart_web_server() -> Result<(), String> {
    info!("Restarting web server");
    
    // Step 1: Graceful shutdown
    let _ = stop_web_server_graceful().await;
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    
    // Step 2: Force kill if still running
    let _ = force_kill_web_server().await;
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    
    // Step 3: Start fresh
    match start_web_server().await {
        Ok(pid) => {
            info!(web_server_pid = pid, "Web server restart completed");
            Ok(())
        }
        Err(e) => {
            error!("Web server restart failed: {}", e);
            Err(e)
        }
    }
}

async fn watch_supervisor(
    session_id: String,
    mut supervisor: Child,
    app_state: AppState,
    cleanup_policy: session_config::CleanupConfig,
    lockdown_enabled: bool,
    backend_url: String,
    backend_key: String,
) {
    info!(
        session_id = %session_id,
        lockdown_mode = lockdown_enabled,
        "Supervisor watcher started"
    );

    let exit_status = match supervisor.wait().await {
        Ok(status) => status,
        Err(e) => {
            error!(
                session_id = %session_id,
                error = %e,
                "Failed to wait for supervisor process"
            );

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

    let reason = ExitReason::from_exit_code(exit_code);

    info!(
        session_id = %session_id,
        exit_reason = %reason,
        "Translated supervisor exit code to reason"
    );

    let should_cleanup = if lockdown_enabled {
        info!(
            session_id = %session_id,
            "Lockdown mode: forcing cleanup"
        );
        true
    } else {
        match reason {
            ExitReason::GameExitedNormally => cleanup_policy.on_normal_exit,
            ExitReason::GameExitedWithError => cleanup_policy.on_normal_exit,
            ExitReason::MaxDurationExceeded => cleanup_policy.on_timeout,
            ExitReason::GameCrashed => cleanup_policy.on_violation,
            ExitReason::IntegrityViolation => cleanup_policy.on_violation,
            ExitReason::LaunchTimeout => cleanup_policy.on_violation,
            ExitReason::TotalFocusLossExceeded => cleanup_policy.on_violation,
            ExitReason::FocusLost => false,
            ExitReason::UnauthorizedProcess => false,
            _ => false,
        }
    };

    if should_cleanup {
        info!(
            session_id = %session_id,
            game_id = %game_id,
            build_id = %build_id,
            reason = %reason,
            lockdown_mode = lockdown_enabled,
            "Cleanup required"
        );

        let strategy = if !cleanup_policy.delete_game_files {
            CleanupStrategy::DeleteSessionOnly
        } else if cleanup_policy.shared_build {
            CleanupStrategy::DeleteSessionOnly
        } else {
            CleanupStrategy::DeleteBuild
        };

        info!(
            session_id = %session_id,
            strategy = ?strategy,
            "Cleanup strategy determined"
        );

        {
            let mut sessions = app_state.sessions.lock().unwrap();
            if let Some(entry) = sessions.get_mut(&session_id) {
                entry.state = SessionState::CleaningUp;
            }
        }

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
        
        // ✨ NEW: Verify cleanup completion
        info!(
            session_id = %session_id,
            "Verifying cleanup completion..."
        );
        
        let mut verification_attempts = 0;
        let max_attempts = 3;
        let mut cleanup_verified = false;
        
        while verification_attempts < max_attempts && !cleanup_verified {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            
            cleanup_verified = verify_cleanup_complete(&game_id, &build_id).await;
            verification_attempts += 1;
            
            if !cleanup_verified {
                warn!(
                    session_id = %session_id,
                    attempt = verification_attempts,
                    "Cleanup verification failed, retrying..."
                );
                
                if verification_attempts < max_attempts {
                    if let Err(e) = force_cleanup(&game_id, &build_id).await {
                        warn!(
                            session_id = %session_id,
                            error = %e,
                            "Force cleanup attempt failed"
                        );
                    }
                }
            }
        }
        
        if cleanup_verified {
            info!(
                session_id = %session_id,
                "Cleanup verification successful"
            );
        } else {
            error!(
                session_id = %session_id,
                "Cleanup verification FAILED after {} attempts, but continuing",
                max_attempts
            );
        }
    } else {
        info!(
            session_id = %session_id,
            reason = %reason,
            "No cleanup required"
        );
    }

    // ✨ NEW: Clean session resources from memory
    cleanup_session_resources(&session_id, &app_state).await;

    // ✨ NEW: Restart web server after cleanup
    // info!(
    //     session_id = %session_id,
    //     "Restarting web server after cleanup"
    // );
    
    // if let Err(e) = restart_web_server().await {
    //     warn!(
    //         session_id = %session_id,
    //         error = %e,
    //         "Web server restart failed (non-blocking)"
    //     );
    // }

    notify_backend(&backend_url, &backend_key, &session_id, "ended_and_ready", None).await;

    {
        let mut sessions = app_state.sessions.lock().unwrap();
        if let Some(entry) = sessions.get_mut(&session_id) {
            entry.state = SessionState::Ended;
            entry.supervisor_handle = None;
        }
        sessions.remove(&session_id);
    }

    info!(
        session_id = %session_id,
        "Supervisor watcher completed, instance ready for reuse"
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

    #[serde(default)]
    pub lockdown_enabled: bool,
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
    info!("START_SESSION ENTERED");
    info!(
        session_id = %req.session_id,
        game_id = %req.game_id,
        build_id = %req.build_id,
        lockdown_enabled = req.lockdown_enabled,
        "Starting session"
    );

    {
        let sessions = state.sessions.lock().unwrap();
        if sessions.contains_key(&req.session_id) {
            return Err(format!("Session {} already exists", req.session_id));
        }
    }

    let build_root = format!("C:\\games\\{}\\{}", req.game_id, req.build_id);
    let exe_path = format!("{}\\game\\{}", build_root, req.start_path);
    let session_dir = Path::new(&build_root).join("session");

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

    notify_backend(
        &req.backend_api_url,
        &req.backend_api_key,
        &req.session_id,
        "provisioning",
        None,
    )
    .await;
    

    info!("BEFORE ensure_game_ready");

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

        {
            let mut sessions = state.sessions.lock().unwrap();
            if let Some(entry) = sessions.get_mut(&req.session_id) {
                entry.state = SessionState::Failed(e.to_string());
            }
        }

        return Err(e.to_string());
    }

    info!("AFTER ensure_game_ready");

    let cleanup_policy = session_config::CleanupConfig {
        on_normal_exit: req.cleanup_on_normal_exit,
        on_violation: req.cleanup_on_violation,
        on_timeout: req.cleanup_on_timeout,
        delete_game_files: req.delete_game_files,
        shared_build: req.shared_build,
    };

    info!("BEFORE write_session_config");

    let session_json = match session_config::write_session_config(
        &session_dir,
        &req.session_id,
        &req.game_id,
        Path::new(&exe_path),
        req.max_duration_seconds,
        &req.backend_api_url,
        &req.backend_api_key,
        cleanup_policy.clone(),
        req.lockdown_enabled,
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

            {
                let mut sessions = state.sessions.lock().unwrap();
                if let Some(entry) = sessions.get_mut(&req.session_id) {
                    entry.state = SessionState::Failed(e.to_string());
                }
            }

            return Err(e.to_string());
        }
    };
    info!("AFTER write_session_config");

    notify_backend(
        &req.backend_api_url,
        &req.backend_api_key,
        &req.session_id,
        "launching",
        None,
    )
    .await;

    info!("BEFORE start_supervisor");

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

            {
                let mut sessions = state.sessions.lock().unwrap();
                if let Some(entry) = sessions.get_mut(&req.session_id) {
                    entry.state = SessionState::Failed(e.to_string());
                }
            }

            return Err(e.to_string());
        }
    };


    info!("AFTER start_supervisor");



    {
        let mut sessions = state.sessions.lock().unwrap();
        if let Some(entry) = sessions.get_mut(&req.session_id) {
            entry.state = SessionState::Launching;
            entry.supervisor_handle = Some(supervisor_child.id().unwrap());
        }
    }

    let state_clone = state.clone();
    let session_id_clone = req.session_id.clone();
    let backend_url_clone = req.backend_api_url.clone();
    let backend_key_clone = req.backend_api_key.clone();
    let lockdown_enabled = req.lockdown_enabled;

    tokio::task::spawn(async move {
        watch_supervisor(
            session_id_clone,
            supervisor_child,
            state_clone,
            cleanup_policy,
            lockdown_enabled,
            backend_url_clone,
            backend_key_clone,
        )
        .await;
    });

    info!(
        session_id = %req.session_id,
        "Session started, supervisor watcher spawned"
    );

    info!("RETURNING HTTP RESPONSE");

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
        entry.state = SessionState::Stopping;
        pid
    };

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

// ✨ NEW: Prepare for next stream endpoint
// #[derive(Deserialize)]
// pub struct PrepareForNextStreamRequest {
//     pub session_id: String,
//     pub force_restart: Option<bool>,
// }

// #[derive(Serialize)]
// pub struct PrepareForNextStreamResponse {
//     pub ready: bool,
//     pub message: String,
// }

// // ✅ FIXED: Correct function signature for Axum handler
// pub async fn prepare_for_next_stream(
//     State(state): State<AppState>,
//     Json(req): Json<PrepareForNextStreamRequest>,
// ) -> Json<PrepareForNextStreamResponse> {
//     info!(
//         session_id = %req.session_id,
//         "Preparing instance for next stream"
//     );

//     let force = req.force_restart.unwrap_or(false);

//     if force {
//         info!("Force restarting web server");
//         let _ = force_kill_web_server().await;
//         tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
//     } else {
//         let _ = stop_web_server_graceful().await;
//         tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
//     }

//     match start_web_server().await {
//         Ok(pid) => {
//             info!(web_server_pid = pid, "Instance prepared for next stream");
//             Json(PrepareForNextStreamResponse {
//                 ready: true,
//                 message: format!("Web server restarted (PID: {})", pid),
//             })
//         }
//         Err(e) => {
//             error!("Failed to restart web server: {}", e);
//             Json(PrepareForNextStreamResponse {
//                 ready: false,
//                 message: format!("Web server restart failed: {}", e),
//             })
//         }
//     }
// }