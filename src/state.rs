use std::{
    collections::HashMap,
    process::Child,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug)]
pub enum SessionState {
    Provisioning,
    Downloading,
    Launching,
    Running,
    Failed(String),
    Ended,
}


pub struct SessionEntry {
    pub state: SessionState,
    pub supervisor: Option<Child>,
}

#[derive(Clone)]
pub struct AppState {
    pub sessions: Arc<Mutex<HashMap<String, SessionEntry>>>,
}
