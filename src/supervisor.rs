use anyhow::{Context, Result};
use std::path::Path;
use tokio::process::{Child, Command};

/// Start supervisor process asynchronously
pub async fn start_supervisor(session_json: &Path) -> Result<Child> {
    Command::new("gamesupervisor.exe")
        .arg(session_json)
        .spawn()
        .context("Failed to start game supervisor")
}

/// Kill supervisor process by PID
pub async fn kill_supervisor(pid: u32) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command as StdCommand;
        
        let output = StdCommand::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()
            .context("Failed to execute taskkill")?;

        if !output.status.success() {
            anyhow::bail!(
                "taskkill failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        use nix::sys::signal::{self, Signal};
        use nix::unistd::Pid;

        signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM)
            .context("Failed to send SIGTERM to supervisor")?;

        Ok(())
    }
}