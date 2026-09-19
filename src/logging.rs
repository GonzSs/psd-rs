// logging.rs — Three-level logging with error state file and BEL alerting.
//
// Error messages always print, emit a terminal BEL (\x07), and write a
// persistent error state file so external monitoring can detect failures.
// Info and debug messages are gated by the configured LogLevel.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Logging verbosity level for psd-rs output.
///
/// Variants are ordered by verbosity: Error < Info < Debug.
/// The derived PartialOrd uses declaration order, so comparisons
/// like `level >= LogLevel::Info` work correctly.
#[derive(Clone, Copy, PartialEq, PartialOrd)]
pub enum LogLevel {
    Error,
    Info,
    Debug,
}

impl LogLevel {
    /// Parse a log level string from the config file.
    /// Accepts "error", "info", or "debug" (case-insensitive).
    pub fn from_str(s: &str) -> Option<LogLevel> {
        match s.to_lowercase().as_str() {
            "error" => Some(LogLevel::Error),
            "info" => Some(LogLevel::Info),
            "debug" => Some(LogLevel::Debug),
            _ => None,
        }
    }
}

/// Global log level, set once at startup from the config file.
static LOG_LEVEL: OnceLock<LogLevel> = OnceLock::new();

/// Initialize the global log level. Called once from run_daemon().
pub fn init(level: LogLevel) {
    let _ = LOG_LEVEL.set(level);
}

/// Returns the current log level. Defaults to Info if not initialized.
pub fn level() -> LogLevel {
    *LOG_LEVEL.get().unwrap_or(&LogLevel::Info)
}

/// Returns the path to the error state file.
///
/// Uses $XDG_RUNTIME_DIR (typically /run/user/$UID on systemd) with
/// a fallback to /tmp for runit or other init systems.
fn error_state_path() -> PathBuf {
    let runtime_dir = env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("psd-rs.error")
}

/// Write the error state file to signal a critical failure.
/// External scripts or shell prompts can check for this file.
pub fn write_error_state(msg: &str) {
    let path = error_state_path();
    let _ = fs::write(&path, msg);
}

/// Clear the error state file on successful startup.
pub fn clear_error_state() {
    let path = error_state_path();
    if path.exists() {
        let _ = fs::remove_file(&path);
    }
}

/// Log an error message. Always printed. Emits BEL and writes error state file.
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        eprintln!("[ERROR] {}", msg);
        eprint!("\x07");
        $crate::logging::write_error_state(&msg);
    }};
}

/// Log an informational message. Printed at Info and Debug levels.
#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {{
        if $crate::logging::level() >= $crate::logging::LogLevel::Info {
            eprintln!("[INFO] {}", format!($($arg)*));
        }
    }};
}

/// Log a debug message. Only printed at Debug level.
#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {{
        if $crate::logging::level() >= $crate::logging::LogLevel::Debug {
            eprintln!("[DEBUG] {}", format!($($arg)*));
        }
    }};
}
