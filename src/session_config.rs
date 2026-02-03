use anyhow::Result;
use serde::Serialize;
use std::{fs, path::{Path, PathBuf}};

#[derive(Serialize)]
pub struct SessionConfig {
    pub session_id: String,
    pub max_duration_seconds: u64,
    pub game: GameConfig,
    pub monitoring: MonitoringConfig,
    pub allowed_processes: Vec<String>,
    pub logging: LoggingConfig,
    pub moonlight: MoonlightConfig,
    pub backend: BackendConfig,
}

#[derive(Serialize)]
pub struct GameConfig {
    pub id: String,
    pub exe: String,
    pub args: Vec<String>,
    pub working_dir: String,
    pub expected_hash: Option<String>,
    pub launch_timeout_seconds: u64,
    pub grace_period_seconds: u64,
}

#[derive(Serialize)]
pub struct MonitoringConfig {
    pub poll_interval_ms: u64,
    pub focus_loss_grace_period_ms: u64,
    pub max_total_focus_loss_seconds: u64,
    pub cpu_threshold_percent: f64,
    pub memory_limit_mb: u64,
}

#[derive(Serialize)]
pub struct LoggingConfig {
    pub log_file: String,
    pub audit_file: String,
    pub verbosity: String,
}

#[derive(Serialize)]
pub struct MoonlightConfig {
    pub service_names: Vec<String>,
    pub process_names: Vec<String>,
    pub terminate_on_violation: bool,
    pub force_terminate: bool,
}

#[derive(Serialize)]
pub struct BackendConfig {
    pub api_url: String,
    pub api_key: String,
    pub timeout_seconds: u64,
    pub retry_attempts: u64,
    pub notify_on_violation: bool,
    pub notify_on_completion: bool,
}

pub fn write_session_config(
    dir: &Path,
    session_id: &str,
    game_id: &str,
    exe: &Path,
    max_duration: u64,
    backend_url: &str,
    backend_key: &str,
) -> Result<PathBuf> {
    fs::create_dir_all(dir.join("logs"))?;

    let cfg = SessionConfig {
        session_id: session_id.into(),
        max_duration_seconds: max_duration,

        game: GameConfig {
            id: game_id.into(),
            exe: exe.display().to_string(),
            args: vec!["-fullscreen".into(), "-noeac".into()],
            working_dir: exe.parent().unwrap().display().to_string(),
            expected_hash: None,
            launch_timeout_seconds: 60,
            grace_period_seconds: 5,
        },

        monitoring: MonitoringConfig {
            poll_interval_ms: 100,
            focus_loss_grace_period_ms: 15_000,
            max_total_focus_loss_seconds: 60,
            cpu_threshold_percent: 95.0,
            memory_limit_mb: 8192,
        },

        allowed_processes: vec![
            "cmd.exe","Game.exe","EasyAntiCheat.exe","EOSOverlayRenderer.exe",
            "nvcontainer.exe","steam.exe","steamservice.exe",
            "steamwebhelper.exe","dxdiag.exe",
        ].into_iter().map(String::from).collect(),

        logging: LoggingConfig {
            log_file: "logs/supervisor.log".into(),
            audit_file: "logs/audit.json".into(),
            verbosity: "info".into(),
        },

        moonlight: MoonlightConfig {
            service_names: vec!["ApolloService".into()],
            process_names: vec!["web-server.exe","sunshine.exe","streamer.exe"]
                .into_iter().map(String::from).collect(),
            terminate_on_violation: true,
            force_terminate: true,
        },

        backend: BackendConfig {
            api_url: backend_url.into(),
            api_key: backend_key.into(),
            timeout_seconds: 10,
            retry_attempts: 3,
            notify_on_violation: true,
            notify_on_completion: true,
        },
    };

    let path = dir.join("session.json");
    fs::write(&path, serde_json::to_vec_pretty(&cfg)?)?;
    Ok(path)
}
