use anyhow::{Result, Context};
use std::process::Command;

pub fn ensure_game_ready(
    game_id: &str,
    build_id: &str,
    s3_url: &str,
    format: &str,
) -> Result<()> {
    let status = Command::new("game_provisioner.exe")
        .args([
            "--game-id", game_id,
            "--build-id", build_id,
            "--s3-url", s3_url,
            "--format", format,
        ])
        .status()
        .context("Failed to start game_provisioner")?;

    if !status.success() {
        anyhow::bail!("Game provisioning failed");
    }

    Ok(())
}
