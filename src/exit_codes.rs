use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum ExitReason {
    GameExitedNormally = 0,
    GameExitedWithError = 1,
    FocusLost = 2,
    UnauthorizedProcess = 3,
    GameCrashed = 4,
    MaxDurationExceeded = 5,
    IntegrityViolation = 6,
    LaunchTimeout = 7,
    GameNotFound = 8,
    ConfigurationError = 9,
    TotalFocusLossExceeded = 10,
    SupervisorError = 100,
}

impl ExitReason {
    /// Convert supervisor exit code to ExitReason
    pub fn from_exit_code(code: i32) -> Self {
        match code {
            0 => ExitReason::GameExitedNormally,
            1 => ExitReason::GameExitedWithError,
            2 => ExitReason::FocusLost,
            3 => ExitReason::UnauthorizedProcess,
            4 => ExitReason::GameCrashed,
            5 => ExitReason::MaxDurationExceeded,
            6 => ExitReason::IntegrityViolation,
            7 => ExitReason::LaunchTimeout,
            8 => ExitReason::GameNotFound,
            9 => ExitReason::ConfigurationError,
            10 => ExitReason::TotalFocusLossExceeded,
            100 => ExitReason::SupervisorError,
            _ => {
                tracing::warn!(code = code, "Unknown exit code, treating as SupervisorError");
                ExitReason::SupervisorError
            }
        }
    }

    pub fn exit_code(&self) -> i32 {
        *self as i32
    }

    pub fn description(&self) -> &'static str {
        match self {
            ExitReason::GameExitedNormally => "Game exited normally",
            ExitReason::GameExitedWithError => "Game exited with error code",
            ExitReason::FocusLost => "Game window lost focus for too long",
            ExitReason::UnauthorizedProcess => "Unauthorized process detected",
            ExitReason::GameCrashed => "Game process crashed",
            ExitReason::MaxDurationExceeded => "Maximum session duration exceeded",
            ExitReason::IntegrityViolation => "Integrity check failed",
            ExitReason::LaunchTimeout => "Game failed to launch within timeout",
            ExitReason::GameNotFound => "Game executable not found",
            ExitReason::ConfigurationError => "Configuration error",
            ExitReason::TotalFocusLossExceeded => "Total cumulative focus loss time exceeded",
            ExitReason::SupervisorError => "Supervisor internal error",
        }
    }

    pub fn is_violation(&self) -> bool {
        matches!(
            self,
            ExitReason::FocusLost
                | ExitReason::UnauthorizedProcess
                | ExitReason::GameCrashed
                | ExitReason::IntegrityViolation
                | ExitReason::LaunchTimeout
                | ExitReason::TotalFocusLossExceeded
                | ExitReason::SupervisorError
        )
    }
}

impl std::fmt::Display for ExitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code: {})", self.description(), self.exit_code())
    }
}