use anyhow::{Result, Context};
use std::process::{Command, Child};
use std::path::Path;

pub fn start_supervisor(session_json: &Path) -> Result<Child> {
    Command::new("gamesupervisor.exe")
        .arg(session_json)
        .spawn()
        .context("Failed to start game supervisor")
}
