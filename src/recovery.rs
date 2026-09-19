// recovery.rs — Crash recovery and split-brain resolution.
//
// After a crash or reboot, browser profiles may be left in an inconsistent
// state: dangling symlinks pointing to evaporated /dev/shm directories, or
// both a profile and backup directory existing simultaneously. This module
// detects and heals both conditions.

use std::fs;
use std::path::Path;

use crate::{cleanup_stale_locks, sanitize_chromium_preferences, BrowserType};
use crate::{log_debug, log_error, log_info};

/// Recover from a dangling symlink after a crash or reboot.
///
/// If the profile path is a symlink whose target no longer exists (the RAM
/// directory evaporated), remove the symlink and restore from the static backup.
/// Returns true if recovery was performed or the profile is already active.
pub fn recover_dangling_symlink(
    full_profile_path: &Path,
    static_backup_path: &Path,
    browser_type: BrowserType,
) -> bool {
    if let Ok(metadata) = fs::symlink_metadata(full_profile_path) {
        if metadata.file_type().is_symlink() {
            if let Ok(target) = fs::read_link(full_profile_path) {
                if !target.exists() {
                    log_info!(
                        "Detected dangling symlink at {:?} (target {:?} gone). Recovering...",
                        full_profile_path,
                        target
                    );

                    // Remove the dangling symlink
                    let _ = fs::remove_file(full_profile_path);

                    // Restore from backup
                    if static_backup_path.exists() {
                        cleanup_stale_locks(static_backup_path);
                        if browser_type == BrowserType::Chromium {
                            sanitize_chromium_preferences(static_backup_path);
                        }
                        if let Err(e) = fs::rename(static_backup_path, full_profile_path) {
                            log_error!("Failed to restore backup directory: {:?}", e);
                            return false;
                        }
                        cleanup_stale_locks(full_profile_path);
                        if browser_type == BrowserType::Chromium {
                            sanitize_chromium_preferences(full_profile_path);
                        }
                        log_info!("Successfully restored profile directory from backup.");
                        return true;
                    }
                } else {
                    log_debug!("Profile is already active in RAM and symlinked correctly.");
                    return true;
                }
            }
        }
    }
    false
}

/// Resolve split-brain state where both profile and backup directories exist.
///
/// This can happen if the browser was launched while the daemon was stopped,
/// creating a new profile directory alongside the existing backup. We preserve
/// the backup (which has the full history) and move the conflicting directory
/// to a -stale suffix for manual inspection.
pub fn recover_split_brain(
    full_profile_path: &Path,
    static_backup_path: &Path,
    leaf_name: &str,
    browser_name: &str,
) -> Result<(), String> {
    let is_symlink = fs::symlink_metadata(full_profile_path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);

    if full_profile_path.exists() && !is_symlink && static_backup_path.exists() {
        log_info!(
            "Detected split-brain state for {}: both profile and backup exist. Resolving...",
            browser_name
        );

        let mut stale_path = full_profile_path.to_path_buf();
        let stale_name = format!("{}-stale", leaf_name);
        stale_path.set_file_name(stale_name);

        if stale_path.exists() {
            let _ = fs::remove_dir_all(&stale_path);
        }

        if let Err(e) = fs::rename(full_profile_path, &stale_path) {
            return Err(format!(
                "Failed to rename conflicting profile directory: {:?}",
                e
            ));
        }
        log_info!("Moved conflicting profile directory to {:?}", stale_path);

        if let Err(e) = fs::rename(static_backup_path, full_profile_path) {
            // Rollback: put the stale directory back
            let _ = fs::rename(&stale_path, full_profile_path);
            return Err(format!("Failed to restore backup directory: {:?}", e));
        }
        log_info!("Successfully restored backup directory to active profile slot.");
    }
    Ok(())
}
