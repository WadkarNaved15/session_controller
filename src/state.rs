use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub struct SessionEntry {
    pub state: SessionState,
    pub supervisor_handle: Option<u32>, // Stores PID, not Child
    pub game_id: String,
    pub build_id: String,
    pub created_at: std::time::Instant,
}

#[derive(Clone, Debug)]
pub enum SessionState {
    Provisioning,
    Downloading,
    Launching,
    Running,
    Stopping,
    CleaningUp,
    Failed(String),
    Ended,
}

#[derive(Clone)]
pub struct AppState {
    pub sessions: Arc<Mutex<HashMap<String, SessionEntry>>>,
}