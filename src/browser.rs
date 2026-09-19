// browser.rs — Browser initialization pipeline.
//
// For each configured browser: run crash/split-brain recovery, verify the
// browser isn't running, move the profile to RAM, and bridge with a symlink.
// This corresponds to the old Phases 1-3 in the monolithic main.rs.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::recovery;
use crate::{
    cleanup_stale_locks, get_volatile_path, is_browser_running, leaf_name, rsync,
    sanitize_chromium_preferences, BrowserConfig, BrowserType,
    log_debug, log_error, log_info,
};

/// Process a single browser: recover, verify, move to RAM, create symlink.
///
/// This is the full initialization pipeline for one browser profile.
/// Errors are logged internally; the function returns without propagating
/// so the daemon can continue processing remaining browsers.
pub fn process_browser(config: &BrowserConfig, volatile_base: &Path, cooldown_delay: u64) {
    log_info!("=== Processing Browser: {} ===", config.name);

    if !config.base_dir.exists() {
        log_info!(
            "Base directory does not exist for {}. Skipping.",
            config.name
        );
        return;
    }

    let mut full_profile_path = config.base_dir.clone();
    full_profile_path.push(&config.profile_dir_name);

    let leaf = leaf_name(&full_profile_path, &config.profile_dir_name);

    let mut static_backup_path = full_profile_path.clone();
    static_backup_path.set_file_name(format!("{}-backup", leaf));

    let volatile_path = get_volatile_path(volatile_base, &config.name, &leaf);

    // --- Crash Recovery ---
    recovery::recover_dangling_symlink(
        &full_profile_path,
        &static_backup_path,
        config.browser_type,
    );

    // --- Split-Brain Recovery ---
    if let Err(e) = recovery::recover_split_brain(
        &full_profile_path,
        &static_backup_path,
        &leaf,
        &config.name,
    ) {
        log_error!("{}: {}", config.name, e);
        return;
    }

    // --- Locate and Verify ---
    if is_browser_running(&full_profile_path, &config.lock_file_name) {
        log_info!(
            "{} is currently running. Skipping to prevent data corruption.",
            config.name
        );
        return;
    }

    // Cooldown settling period: if stale lock files linger, the browser may have
    // just exited. Sleep briefly to let the OS page cache and SQLite WAL flush.
    let lock_path = full_profile_path.join(&config.lock_file_name);
    let parentlock_path = full_profile_path.join(".parentlock");
    let has_stale_lock = fs::symlink_metadata(&lock_path).is_ok() || parentlock_path.exists();
    if has_stale_lock {
        log_info!(
            "Detected stale lock files for {}. Waiting {}ms for filesystem to settle...",
            config.name,
            cooldown_delay
        );
        thread::sleep(Duration::from_millis(cooldown_delay));
    }

    // Already symlinked to RAM?
    if let Ok(metadata) = fs::symlink_metadata(&full_profile_path) {
        if metadata.file_type().is_symlink() {
            log_info!(
                "Profile for {} is already active in RAM. Skipping.",
                config.name
            );
            return;
        }
    }

    // --- Move to RAM ---
    if !full_profile_path.exists() {
        log_info!("No profile found at {:?}. Skipping.", full_profile_path);
        return;
    }

    // Clean leftover backup so rename succeeds
    if static_backup_path.exists() {
        let _ = fs::remove_dir_all(&static_backup_path);
    }

    log_info!(
        "Renaming physical profile to backup location: {:?}",
        static_backup_path
    );
    if let Err(e) = fs::rename(&full_profile_path, &static_backup_path) {
        log_error!(
            "Failed to rename profile directory {:?}: {:?}",
            full_profile_path,
            e
        );
        return;
    }

    cleanup_stale_locks(&static_backup_path);
    if config.browser_type == BrowserType::Chromium {
        sanitize_chromium_preferences(&static_backup_path);
    }

    log_debug!("Creating RAM directory at {:?}", volatile_path);
    if let Err(e) = fs::create_dir_all(&volatile_path) {
        log_error!("Failed to create RAM directory {:?}: {:?}", volatile_path, e);
        let _ = fs::rename(&static_backup_path, &full_profile_path);
        return;
    }

    log_info!("Syncing files to RAM (excluding cache)...");
    if let Err(e) = rsync(&static_backup_path, &volatile_path, &config.exclude_patterns) {
        log_error!("rsync failed for {}: {}", config.name, e);
        let _ = fs::rename(&static_backup_path, &full_profile_path);
        return;
    }

    cleanup_stale_locks(&volatile_path);
    if config.browser_type == BrowserType::Chromium {
        sanitize_chromium_preferences(&volatile_path);
    }

    // --- Bridge with Symlink ---
    // Ensure the target path is clear before creating the symlink
    if full_profile_path.exists() || fs::symlink_metadata(&full_profile_path).is_ok() {
        let _ = fs::remove_file(&full_profile_path);
        let _ = fs::remove_dir_all(&full_profile_path);
    }

    log_debug!("Creating symlink bridge...");
    if let Err(e) = symlink(&volatile_path, &full_profile_path) {
        log_error!(
            "Failed to create symlink {:?} -> {:?}: {:?}",
            full_profile_path,
            volatile_path,
            e
        );
        let _ = fs::rename(&static_backup_path, &full_profile_path);
        return;
    }

    log_info!("Success! {} profile is running from RAM.", config.name);
}
