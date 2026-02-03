use axum::{Json, extract::State};
use serde::Deserialize;
use std::path::Path;

use crate::{
    state::{AppState, SessionEntry, SessionState},
    provisioner,
    supervisor,
    session_config,
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
    });

    let _ = reqwest::Client::new()
        .post(format!("{}/api/internal/sessions/update", api_url))
        .header("Authorization", api_key)
        .json(&payload)
        .send()
        .await;
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
}

pub async fn start_session(
    State(state): State<AppState>,
    Json(req): Json<StartSessionRequest>,
) -> Result<Json<&'static str>, String> {

    let build_root = format!("C:\\games\\{}\\{}", req.game_id, req.build_id);
    let exe_path = format!("{}\\game\\{}", build_root, req.start_path);
    let session_dir = Path::new(&build_root).join("session");

    {
        state.sessions.lock().unwrap().insert(
            req.session_id.clone(),
            SessionEntry {
                state: SessionState::Provisioning,
                supervisor: None,
            },
        );
    }

notify_backend(&req.backend_api_url, &req.backend_api_key, &req.session_id, "downloading", None).await;

if let Err(e) = provisioner::ensure_game_ready(
    &req.game_id,
    &req.build_id,
    &req.s3_url,
    &req.format,
) {
    notify_backend(
        &req.backend_api_url,
        &req.backend_api_key,
        &req.session_id,
        "failed",
        Some(&format!("Provisioning failed: {}", e)),
    ).await;

    return Err(e.to_string());
}


let session_json = match session_config::write_session_config(
    &session_dir,
    &req.session_id,
    &req.game_id,
    Path::new(&exe_path),
    req.max_duration_seconds,
    &req.backend_api_url,
    &req.backend_api_key,
) {
    Ok(p) => p,
    Err(e) => {
        notify_backend(
            &req.backend_api_url,
            &req.backend_api_key,
            &req.session_id,
            "failed",
            Some(&format!("Session config error: {}", e)),
        ).await;

        return Err(e.to_string());
    }
};


    
notify_backend(&req.backend_api_url, &req.backend_api_key, &req.session_id, "launching", None).await;

let child = match supervisor::start_supervisor(&session_json) {
    Ok(c) => c,
    Err(e) => {
        notify_backend(
            &req.backend_api_url,
            &req.backend_api_key,
            &req.session_id,
            "failed",
            Some(&format!("Supervisor failed to start: {}", e)),
        ).await;

        return Err(e.to_string());
    }
};


notify_backend(&req.backend_api_url, &req.backend_api_key, &req.session_id, "running", None).await;


    let mut sessions = state.sessions.lock().unwrap();
    let entry = sessions.get_mut(&req.session_id).unwrap();
    entry.supervisor = Some(child);
    entry.state = SessionState::Running;

    Ok(Json("started"))
}

#[derive(Deserialize)]
pub struct StopSessionRequest {
    pub session_id: String,
}

pub async fn stop_session(
    State(state): State<AppState>,
    Json(req): Json<StopSessionRequest>,
) -> Result<Json<&'static str>, String> {

    let mut sessions = state.sessions.lock().unwrap();
    let entry = sessions.get_mut(&req.session_id).ok_or("Session not found")?;
    if let Some(child) = entry.supervisor.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }

    entry.supervisor = None;
    entry.state = SessionState::Ended;

    Ok(Json("stopped"))
}
