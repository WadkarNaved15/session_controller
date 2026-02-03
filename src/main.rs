mod api;
mod provisioner;
mod session_config;
mod supervisor;
mod state;

use axum::{Router, routing::{post, get}};
use state::AppState;
use std::{collections::HashMap, sync::{Arc, Mutex}};
use axum::Json;
use serde_json::json;
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
        .route("/health", get(|| async {
                Json(json!({
                    "status": "ok",
                    "currentSessions": 0
                }))
            }))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:4443")
        .await
        .expect("Failed to bind");

    println!("✅ Session Controller running on :4443");
    axum::serve(listener, app)
        .await
        .expect("Server crashed");
}
