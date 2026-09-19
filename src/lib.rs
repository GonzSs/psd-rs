// lib.rs — psd-rs crate root.
//
// Declares the module tree, defines shared types (BrowserConfig, BrowserType),
// provides shared helper functions used across modules, and exposes
// run_daemon() as the main entry point for the daemon lifecycle.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// --- Module declarations ---
pub mod browser;
pub mod config;
pub mod logging;
pub mod recovery;
pub mod sync;

// --- Shared types ---

/// The type of browser engine, determining lock detection and cleanup behavior.
#[derive(Clone, Copy, PartialEq)]
pub enum BrowserType {
    /// Chromium-based: SingletonLock, Preferences JSON sanitization.
    Chromium,
    /// Gecko-based (Firefox family): profiles.ini, .parentlock handling.
    Gecko,
    /// Generic: plain rsync, no browser-specific logic.
    Generic,
}

/// Configuration for a single managed browser profile.
pub struct BrowserConfig {
    pub name: String,
    pub base_dir: PathBuf,
    pub profile_dir_name: String,
    pub lock_file_name: String,
    pub exclude_patterns: Vec<String>,
    pub browser_type: BrowserType,
}

// --- Shared helper functions ---

/// Construct the volatile RAM profile path in a multi-user safe manner.
///
/// Produces paths like: /dev/shm/username-firefox-arp45dlc.Nihil
pub fn get_volatile_path(volatile_base: &Path, browser_name: &str, profile_leaf: &str) -> PathBuf {
    let username = env::var("USER")
        .or_else(|_| env::var("LOGNAME"))
        .unwrap_or_else(|_| "shared".to_string());
    let sanitized_leaf = profile_leaf.replace('/', "-");
    volatile_base.join(format!(
        "{}-{}-{}",
        username,
        browser_name.to_lowercase(),
        sanitized_leaf
    ))
}

/// Clean up any leftover lock symlinks or socket files in a profile directory.
pub fn cleanup_stale_locks(profile_path: &Path) {
    let stale_files = [
        "SingletonLock",
        "SingletonSocket",
        "lockfile",
        "lock",
        ".parentlock",
    ];
    for file in &stale_files {
        let p = profile_path.join(file);
        if p.exists() || fs::symlink_metadata(&p).is_ok() {
            let _ = fs::remove_file(&p);
        }
    }
}

/// Sanitize Chromium's Preferences file so that exit_type is marked as Normal
/// and exited_cleanly is true.
///
/// This prevents Chromium from showing "Closed unexpectedly" banners upon
/// launch. Only called during crash recovery, never during live sync.
pub fn sanitize_chromium_preferences(profile_path: &Path) {
    let pref_path = profile_path.join("Preferences");
    if pref_path.exists() {
        if let Ok(content) = fs::read_to_string(&pref_path) {
            let sanitized = content
                .replace("\"exit_type\":\"Crashed\"", "\"exit_type\":\"Normal\"")
                .replace("\"exit_type\": \"Crashed\"", "\"exit_type\": \"Normal\"")
                .replace("\"exit_type\":\"SessionEnded\"", "\"exit_type\":\"Normal\"")
                .replace(
                    "\"exit_type\": \"SessionEnded\"",
                    "\"exit_type\":\"Normal\"",
                )
                .replace("\"exited_cleanly\":false", "\"exited_cleanly\":true")
                .replace("\"exited_cleanly\": false", "\"exited_cleanly\": true");

            if sanitized != content {
                let _ = fs::write(&pref_path, sanitized);
            }
        }
    }
}

/// Inspect the browser's lock file/symlink and verify process liveness.
///
/// If the lock exists but the process is dead, self-heal by removing the
/// stale lock files. Returns true only if a live browser process is found.
pub fn is_browser_running(profile_path: &Path, lock_file: &str) -> bool {
    let lock_path = profile_path.join(lock_file);

    if let Ok(target) = fs::read_link(&lock_path) {
        if let Some(target_str) = target.to_str() {
            // Robustly extract the PID from the end of the symlink target.
            // Works for Firefox (127.0.0.1:+PID), Chromium (host-PID), and raw PIDs.
            let mut pid_str = String::new();
            for c in target_str.chars().rev() {
                if c.is_ascii_digit() {
                    pid_str.insert(0, c);
                } else if !pid_str.is_empty() {
                    break;
                }
            }

            if let Ok(pid) = pid_str.parse::<i32>() {
                let proc_path = format!("/proc/{}", pid);
                let comm_path = format!("/proc/{}/comm", pid);

                if fs::metadata(&proc_path).is_ok() {
                    if let Ok(comm) = fs::read_to_string(&comm_path) {
                        let comm_lower = comm.to_lowercase();
                        // Match firefox, chrome, or brave binaries
                        if comm_lower.contains("firefox")
                            || comm_lower.contains("geckomain")
                            || comm_lower.contains("chrome")
                            || comm_lower.contains("brave")
                        {
                            return true;
                        }
                    }
                }
            }
        }

        // Lock exists but process is dead: self-heal
        log_info!(
            "Detected stale lock symlink {:?}. Cleaning up...",
            lock_path
        );
        let _ = fs::remove_file(&lock_path);
        let _ = fs::remove_file(profile_path.join(".parentlock"));
    } else {
        // Fallback check for orphaned lock files without symlinks
        let parentlock = profile_path.join(".parentlock");
        if parentlock.exists() {
            log_info!(
                "Detected orphaned .parentlock file in {:?}. Cleaning up...",
                profile_path
            );
            let _ = fs::remove_file(&parentlock);
        }
    }
    false
}

/// Extract the leaf folder name from a profile path.
///
/// For a path like `/home/user/.mozilla/firefox/arp45dlc.Nihil`,
/// returns `"arp45dlc.Nihil"`. Falls back to the provided string
/// if the path has no file_name component.
pub fn leaf_name(full_profile_path: &Path, fallback: &str) -> String {
    full_profile_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(fallback)
        .to_string()
}

/// Run rsync to synchronize two directories.
///
/// Uses -a (archive mode), --delete, and --delete-excluded flags.
/// Excludes are passed as --exclude=<pattern> arguments.
pub fn rsync(src: &Path, dst: &Path, excludes: &[String]) -> Result<(), String> {
    let mut cmd = Command::new("rsync");
    cmd.arg("-a").arg("--delete").arg("--delete-excluded");
    for exclude in excludes {
        cmd.arg(format!("--exclude={}", exclude));
    }
    let status = cmd
        .arg(format!("{}/", src.display()))
        .arg(format!("{}/", dst.display()))
        .status()
        .map_err(|e| format!("Failed to execute rsync: {}", e))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("rsync exited with status: {}", status))
    }
}

// --- Daemon entry point ---

/// Main daemon lifecycle: initialize browsers, enter sync loop, handle shutdown.
pub fn run_daemon(config: config::PsdConfig) -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    logging::init(config.log_level);
    logging::clear_error_state();

    log_info!("psd-rs v{} starting", env!("CARGO_PKG_VERSION"));

    if config.browsers.is_empty() {
        return Err("No browsers configured. Check your config file.".into());
    }

    // Initialize all configured browsers (recovery + move to RAM + symlink)
    for browser_config in &config.browsers {
        browser::process_browser(browser_config, &config.volatile_path, config.cooldown_delay);
    }

    // Signal handler for graceful shutdown
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = running.clone();
    if let Err(e) = ctrlc::set_handler(move || {
        eprintln!("\n[INFO] Termination signal received. Shutting down gracefully...");
        r.store(false, std::sync::atomic::Ordering::SeqCst);
    }) {
        log_error!("Failed to set signal handler: {:?}", e);
    }

    log_info!(
        "Daemon started. Syncing every {}s.",
        config.sync_interval
    );

    // Sync loop: sleep in 1-second ticks, sync at the configured interval
    let mut seconds_counter: u64 = 0;

    while running.load(std::sync::atomic::Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_secs(1));
        seconds_counter += 1;

        if seconds_counter >= config.sync_interval {
            seconds_counter = 0;
            log_info!("--- Periodic sync triggered ---");
            for browser_config in &config.browsers {
                sync::sync_volatile_to_backup(browser_config, &config.volatile_path);
            }
        }
    }

    // Graceful shutdown
    log_info!("Initiating graceful shutdown...");
    for browser_config in &config.browsers {
        sync::restore_profile_to_disk(
            browser_config,
            &config.volatile_path,
            config.shutdown_grace_period,
        );
    }

    log_info!("Daemon shutdown complete. Goodbye!");
    Ok(())
}
