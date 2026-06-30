mod api;
mod cleanup;
mod exit_codes;
mod provisioner;
mod session_config;
mod state;
mod supervisor;

use axum::{
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use state::AppState;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tracing_subscriber;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let state = AppState {
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/start-session", post(api::start_session))
        .route("/stop-session", post(api::stop_session))
        .route("/list-sessions", get(api::list_sessions))
        // ✨ NEW: Endpoints for web server restart
        // .route("/prepare-for-next-stream", post(api::prepare_for_next_stream))
        .route("/health", get(|| async {
            Json(json!({
                "status": "ok",
                "service": "session_controller"
            }))
        }))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:4443")
        .await
        .expect("Failed to bind");

    println!("✅ Session Controller running on :4443");
    axum::serve(listener, app).await.expect("Server crashed");
}