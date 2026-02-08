// session_controller/src/cleanup.rs

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::fs;
use tracing::{info, warn};

const GAME_ROOT: &str = "C:\\games";

#[derive(Debug, Clone, Copy)]
pub enum CleanupStrategy {
    DeleteBuild,
    DeleteSessionOnly,
    KeepAll,
}

pub fn cleanup_session(
    game_id: &str,
    build_id: &str,
    strategy: CleanupStrategy,
) -> Result<()> {
    let build_dir = Path::new(GAME_ROOT).join(game_id).join(build_id);
    
    match strategy {
        CleanupStrategy::DeleteBuild => {
            info!(
                game_id = %game_id,
                build_id = %build_id,
                "Deleting entire build directory"
            );
            
            if build_dir.exists() {
                fs::remove_dir_all(&build_dir)?;
            }
        }
        
        CleanupStrategy::DeleteSessionOnly => {
            info!(
                game_id = %game_id,
                build_id = %build_id,
                "Cleaning session-specific files only"
            );
            
            let session_dir = build_dir.join("session");
            if session_dir.exists() {
                fs::remove_dir_all(&session_dir)?;
            }
        }
        
        CleanupStrategy::KeepAll => {
            info!("No cleanup performed (KeepAll strategy)");
        }
    }
    
    Ok(())
}