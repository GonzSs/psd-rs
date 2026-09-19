// sync.rs — Periodic synchronization and graceful shutdown.
//
// sync_volatile_to_backup: rsync RAM profile back to the SSD backup.
//   Called periodically from the daemon's main loop.
//
// restore_profile_to_disk: final sync, remove symlink, rename backup back
//   to original path, clean up RAM directory. Called on SIGTERM/SIGINT.

use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::{
    cleanup_stale_locks, get_volatile_path, is_browser_running, leaf_name, rsync, BrowserConfig,
    log_debug, log_error, log_info,
};

/// Sync the volatile RAM copy back to the static SSD backup.
///
/// Called periodically by the daemon loop. If either the volatile or
/// backup directory is missing, the sync is silently skipped.
pub fn sync_volatile_to_backup(config: &BrowserConfig, volatile_base: &Path) {
    let mut full_profile_path = config.base_dir.clone();
    full_profile_path.push(&config.profile_dir_name);

    let leaf = leaf_name(&full_profile_path, &config.profile_dir_name);

    let mut static_backup_path = full_profile_path.clone();
    static_backup_path.set_file_name(format!("{}-backup", leaf));

    let volatile_path = get_volatile_path(volatile_base, &config.name, &leaf);

    if volatile_path.exists() && static_backup_path.exists() {
        log_info!("Syncing {} from RAM back to SSD backup...", config.name);

        match rsync(&volatile_path, &static_backup_path, &config.exclude_patterns) {
            Ok(()) => {
                cleanup_stale_locks(&static_backup_path);
                log_info!("Successfully synced {} back to SSD.", config.name);
            }
            Err(e) => {
                log_error!("Failed to sync {} back to SSD: {}", config.name, e);
            }
        }
    }
}

/// Restore a browser profile from RAM back to disk during graceful shutdown.
///
/// Waits up to `grace_period_ms` for the browser to finish shutting down,
/// performs a final sync, removes the symlink, renames the backup back to
/// the original path, and cleans up the RAM directory.
pub fn restore_profile_to_disk(
    config: &BrowserConfig,
    volatile_base: &Path,
    grace_period_ms: u64,
) {
    log_info!("=== Restoring {} Profile to Disk ===", config.name);

    let mut full_profile_path = config.base_dir.clone();
    full_profile_path.push(&config.profile_dir_name);

    let leaf = leaf_name(&full_profile_path, &config.profile_dir_name);

    let mut static_backup_path = full_profile_path.clone();
    static_backup_path.set_file_name(format!("{}-backup", leaf));

    let volatile_path = get_volatile_path(volatile_base, &config.name, &leaf);

    // Wait for browser to finish shutting down.
    // Check every 100ms for up to grace_period_ms total.
    let check_interval_ms: u64 = 100;
    let max_attempts = grace_period_ms / check_interval_ms;
    let mut wait_attempts: u64 = 0;

    while is_browser_running(&full_profile_path, &config.lock_file_name)
        && wait_attempts < max_attempts
    {
        log_debug!(
            "Browser {} is still shutting down. Waiting...",
            config.name
        );
        thread::sleep(Duration::from_millis(check_interval_ms));
        wait_attempts += 1;
    }

    if is_browser_running(&full_profile_path, &config.lock_file_name) {
        log_error!(
            "{} is still running after grace period! Refusing to restore to prevent data corruption.",
            config.name
        );
        return;
    }

    // Final sync from RAM to backup
    if volatile_path.exists() && static_backup_path.exists() {
        log_info!("Performing final sync for {}...", config.name);
        if let Err(e) = rsync(&volatile_path, &static_backup_path, &config.exclude_patterns) {
            log_error!("Final sync failed for {}: {}", config.name, e);
        }
        cleanup_stale_locks(&static_backup_path);
    }

    // Remove the symlink bridge
    if let Ok(metadata) = fs::symlink_metadata(&full_profile_path) {
        if metadata.file_type().is_symlink() {
            log_info!("Removing symlink bridge for {}...", config.name);
            let _ = fs::remove_file(&full_profile_path);
        }
    }

    // Rename backup folder back to original profile directory path
    if static_backup_path.exists() {
        log_info!("Restoring backup folder to original path...");
        if let Err(e) = fs::rename(&static_backup_path, &full_profile_path) {
            log_error!(
                "Error restoring backup for {}: {:?}",
                config.name,
                e
            );
        } else {
            cleanup_stale_locks(&full_profile_path);
            log_info!("Successfully restored {} profile to SSD.", config.name);
        }
    }

    // Clean up volatile folder in /dev/shm
    if volatile_path.exists() {
        log_debug!("Cleaning up RAM directory for {}...", config.name);
        let _ = fs::remove_dir_all(&volatile_path);
    }
}
