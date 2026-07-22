use std::env;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy)]
enum LockFormat {
    Firefox,  // Extracts PID after '+' (e.g., 127.0.0.1:+18085)
    Chromium, // Extracts PID after '-' (e.g., hostname-18085)
}

struct BrowserConfig {
    name: &'static str,
    // The directory where profiles sit (e.g. ~/.config/BraveSoftware/Brave-Origin-Beta)
    base_dir: PathBuf,
    // The specific profile folder (e.g. Default)
    profile_dir_name: String,
    lock_file_name: &'static str,
    lock_format: LockFormat,
    exclude_patterns: &'static [&'static str],
}

/// Cleans up any leftover lock symlinks or socket files in a profile directory.
fn cleanup_stale_locks(profile_path: &Path) {
    let stale_files = [
        "SingletonLock",
        "SingletonCookie",
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

/// Sanitizes Chromium's Preferences file so that exit_type is marked as Normal
/// and exited_cleanly is true. This prevents Chromium from showing "Closed unexpectedly"
/// banners or invalidating session security cookies upon launch.
fn sanitize_chromium_preferences(profile_path: &Path) {
    let pref_path = profile_path.join("Preferences");
    if pref_path.exists() {
        if let Ok(content) = fs::read_to_string(&pref_path) {
            let sanitized = content
                .replace("\"exit_type\":\"Crashed\"", "\"exit_type\":\"Normal\"")
                .replace("\"exit_type\": \"Crashed\"", "\"exit_type\": \"Normal\"")
                .replace("\"exit_type\":\"SessionEnded\"", "\"exit_type\":\"Normal\"")
                .replace("\"exit_type\": \"SessionEnded\"", "\"exit_type\": \"Normal\"")
                .replace("\"exited_cleanly\":false", "\"exited_cleanly\":true")
                .replace("\"exited_cleanly\": false", "\"exited_cleanly\": true");

            if sanitized != content {
                let _ = fs::write(&pref_path, sanitized);
            }
        }
    }
}

/// Parses Firefox's profiles.ini file and extracts the default profile path
/// using a zero-allocation parsing strategy over a single contiguous string slice.
fn find_firefox_default_profile(ini_content: &str) -> Option<&str> {
    let mut in_install_section = false;
    for line in ini_content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        
        if line.starts_with('[') && line.ends_with(']') {
            let section = &line[1..line.len() - 1];
            in_install_section = section.len() >= 7 && section[..7].eq_ignore_ascii_case("install");
        } else if in_install_section {
            if let Some(eq_idx) = line.find('=') {
                let key = line[..eq_idx].trim();
                let val = line[eq_idx + 1..].trim();
                if key.eq_ignore_ascii_case("default") {
                    return Some(val);
                }
            }
        }
    }
    None
}

/// Inspects the browser's lock file/symlink and verifies process liveness.
/// If the lock is stale (process is dead), it self-heals by deleting the lock files.
fn is_browser_running(profile_path: &Path, lock_file: &str, format: LockFormat) -> bool {
    let lock_path = profile_path.join(lock_file);
    
    if let Ok(target) = fs::read_link(&lock_path) {
        if let Some(target_str) = target.to_str() {
            // Find the delimiter character based on the browser type
            let delimiter = match format {
                LockFormat::Firefox => '+',
                LockFormat::Chromium => '-',
            };
            
            if let Some(delim_idx) = target_str.rfind(delimiter) {
                let pid_str = &target_str[delim_idx + 1..];
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
                                || comm_lower.contains("brave") {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        
        // Lock exists but process is dead: Self-heal
        println!("Detected stale lock symlink {:?}. Cleaning up...", lock_path);
        let _ = fs::remove_file(&lock_path);
        let _ = fs::remove_file(profile_path.join(".parentlock")); // Firefox-specific secondary lock
    } else {
        // Fallback check for orphaned locking files without symlinks
        let parentlock = profile_path.join(".parentlock");
        if parentlock.exists() {
            println!("Detected orphaned .parentlock file in {:?}. Cleaning up...", profile_path);
            let _ = fs::remove_file(&parentlock);
        }
    }
    false
}

/// Re-integrates the backup and RAM profile, checking for dangling symlinks (e.g. after a crash/reboot).
/// Returns true if a backup exists and was successfully restored.
fn check_and_recover_dangling_symlink(full_profile_path: &Path, static_backup_path: &Path) -> bool {
    if let Ok(metadata) = fs::symlink_metadata(full_profile_path) {
        if metadata.file_type().is_symlink() {
            // Check if the destination target exists
            if let Ok(target) = fs::read_link(full_profile_path) {
                if !target.exists() {
                    println!(
                        "Detected dangling symlink at {:?} (target {:?} does not exist). Recovering from backup...",
                        full_profile_path, target
                    );
                    
                    // 1. Remove the dangling symlink
                    let _ = fs::remove_file(full_profile_path);
                    
                    // 2. Restore physical directory from backup if it exists
                    if static_backup_path.exists() {
                        cleanup_stale_locks(static_backup_path);
                        sanitize_chromium_preferences(static_backup_path);
                        fs::rename(static_backup_path, full_profile_path)
                            .expect("Failed to restore profile from backup directory");
                        cleanup_stale_locks(full_profile_path);
                        sanitize_chromium_preferences(full_profile_path);
                        println!("Successfully restored profile directory from backup.");
                        return true;
                    }
                } else {
                    println!("Profile is already active in RAM and symlinked correctly.");
                    return true;
                }
            }
        }
    }
    false
}

fn process_browser(config: &BrowserConfig) {
    println!("=== Processing Browser: {} ===", config.name);
    
    // Ensure base directory exists
    if !config.base_dir.exists() {
        println!("Base directory does not exist for {}. Skipping.", config.name);
        return;
    }

    let mut full_profile_path = config.base_dir.clone();
    full_profile_path.push(&config.profile_dir_name);

    let mut static_backup_path = full_profile_path.clone();
    let backup_name = format!("{}-backup", config.profile_dir_name);
    static_backup_path.set_file_name(backup_name);

    let volatile_path = PathBuf::from(format!("/dev/shm/{}-{}", config.name.to_lowercase(), config.profile_dir_name));

    // --- CRASH RECOVERY (Self-Healing) ---
    // If the system crashed, the symlink is dangling. This self-heals by restoring from the backup.
    if check_and_recover_dangling_symlink(&full_profile_path, &static_backup_path) {
        // If we restored it, let's proceed to set it up in RAM again.
    }

    // --- SPLIT-BRAIN RECOVERY ---
    // If both the profile directory and the backup directory exist, and the profile is NOT a symlink,
    // the browser was likely launched while the daemon was stopped, creating a new empty profile directory.
    let is_symlink = fs::symlink_metadata(&full_profile_path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false);
    if full_profile_path.exists() && !is_symlink && static_backup_path.exists() {
        println!(
            "Detected split-brain state for {}: both profile and backup exist. Resolving...",
            config.name
        );
        let mut stale_path = full_profile_path.clone();
        let stale_name = format!("{}-stale", config.profile_dir_name);
        stale_path.set_file_name(stale_name);

        if stale_path.exists() {
            let _ = fs::remove_dir_all(&stale_path);
        }
        if let Err(e) = fs::rename(&full_profile_path, &stale_path) {
            eprintln!("Failed to rename conflicting profile directory: {:?}", e);
            return;
        }
        println!("Moved conflicting profile directory to {:?}", stale_path);

        if let Err(e) = fs::rename(&static_backup_path, &full_profile_path) {
            eprintln!("Failed to restore backup directory: {:?}", e);
            let _ = fs::rename(&stale_path, &full_profile_path);
            return;
        }
        println!("Successfully restored backup directory to active profile slot.");
    }

    // --- PHASE 1: LOCATE AND VERIFY ---
    if is_browser_running(&full_profile_path, config.lock_file_name, config.lock_format) {
        println!("{} is currently running. Skipping to prevent data corruption.", config.name);
        return;
    }

    // Safety check: is it already a valid symlink to RAM?
    if let Ok(metadata) = fs::symlink_metadata(&full_profile_path) {
        if metadata.file_type().is_symlink() {
            println!("Profile for {} is already active in RAM. Skipping Phase 2 & 3.", config.name);
            return;
        }
    }

    // --- PHASE 2: MOVE TO RAM ---
    if full_profile_path.exists() {
        println!("Renaming physical profile to backup location: {:?}", static_backup_path);
        fs::rename(&full_profile_path, &static_backup_path)
            .expect("Failed to rename profile directory");
    } else {
        println!("No profile found at {:?}. Skipping.", full_profile_path);
        return;
    }

    // Sanitize locks/preferences in static backup before creating RAM copy
    cleanup_stale_locks(&static_backup_path);
    sanitize_chromium_preferences(&static_backup_path);

    println!("Creating RAM directory at {:?}", volatile_path);
    fs::create_dir_all(&volatile_path).expect("Failed to create RAM directory");

    println!("Syncing files to RAM (excluding cache)...");
    let mut rsync_cmd = Command::new("rsync");
    rsync_cmd.arg("-a").arg("--delete").arg("--delete-excluded");
    for exclude in config.exclude_patterns {
        rsync_cmd.arg(format!("--exclude={}", exclude));
    }
    let status = rsync_cmd
        .arg(format!("{}/", static_backup_path.display()))
        .arg(format!("{}/", volatile_path.display()))
        .status()
        .expect("Failed to execute rsync command");

    if !status.success() {
        println!("rsync failed to copy files to RAM for {}.", config.name);
        // Rollback backup renaming on failure
        let _ = fs::rename(&static_backup_path, &full_profile_path);
        return;
    }

    // Ensure RAM profile has no stale locks or crashed preferences flag
    cleanup_stale_locks(&volatile_path);
    sanitize_chromium_preferences(&volatile_path);

    // --- PHASE 3: BRIDGE WITH A SYMLINK ---
    println!("Creating symlink bridge...");
    symlink(&volatile_path, &full_profile_path).expect("Failed to create symlink");

    println!("Success! {} profile is running from RAM.", config.name);
}

// --- PHASE 4: SYNC LOOP FUNCTION ---
fn sync_volatile_to_backup(config: &BrowserConfig) {
    let mut full_profile_path = config.base_dir.clone();
    full_profile_path.push(&config.profile_dir_name);

    let mut static_backup_path = full_profile_path.clone();
    let backup_name = format!("{}-backup", config.profile_dir_name);
    static_backup_path.set_file_name(backup_name);

    let volatile_path = PathBuf::from(format!("/dev/shm/{}-{}", config.name.to_lowercase(), config.profile_dir_name));

    if volatile_path.exists() && static_backup_path.exists() {
        println!("Syncing {} from RAM back to SSD backup...", config.name);
        
        let mut rsync_cmd = Command::new("rsync");
        rsync_cmd.arg("-a").arg("--delete").arg("--delete-excluded");
        
        for exclude in config.exclude_patterns {
            rsync_cmd.arg(format!("--exclude={}", exclude));
        }
        
        let status = rsync_cmd
            .arg(format!("{}/", volatile_path.display()))
            .arg(format!("{}/", static_backup_path.display()))
            .status();
            
        match status {
            Ok(s) if s.success() => {
                sanitize_chromium_preferences(&static_backup_path);
                cleanup_stale_locks(&static_backup_path);
                println!("Successfully synced {} back to SSD.", config.name);
            }
            _ => eprintln!("Warning: Failed to sync {} back to SSD.", config.name),
        }
    }
}

// --- PHASE 5: GRACEFUL SHUTDOWN FUNCTION ---
fn restore_profile_to_disk(config: &BrowserConfig) {
    println!("=== Restoring {} Profile to Disk (Phase 5) ===", config.name);
    
    let mut full_profile_path = config.base_dir.clone();
    full_profile_path.push(&config.profile_dir_name);

    let mut static_backup_path = full_profile_path.clone();
    let backup_name = format!("{}-backup", config.profile_dir_name);
    static_backup_path.set_file_name(backup_name);

    let volatile_path = PathBuf::from(format!("/dev/shm/{}-{}", config.name.to_lowercase(), config.profile_dir_name));

    // Wait briefly if browser is still shutting down during OS reboot
    let mut wait_attempts = 0;
    while is_browser_running(&full_profile_path, config.lock_file_name, config.lock_format) && wait_attempts < 6 {
        println!("Browser {} is still shutting down. Waiting for process exit...", config.name);
        thread::sleep(Duration::from_millis(500));
        wait_attempts += 1;
    }

    if is_browser_running(&full_profile_path, config.lock_file_name, config.lock_format) {
        eprintln!(
            "Error: {} is still running after grace period! Refusing to restore to disk to prevent data corruption.",
            config.name
        );
        return;
    }

    // 1. Final sync from RAM to backup folder
    if volatile_path.exists() && static_backup_path.exists() {
        println!("Performing final sync for {}...", config.name);
        let mut rsync_cmd = Command::new("rsync");
        rsync_cmd.arg("-a").arg("--delete").arg("--delete-excluded");
        for exclude in config.exclude_patterns {
            rsync_cmd.arg(format!("--exclude={}", exclude));
        }
        let _ = rsync_cmd
            .arg(format!("{}/", volatile_path.display()))
            .arg(format!("{}/", static_backup_path.display()))
            .status();

        sanitize_chromium_preferences(&static_backup_path);
        cleanup_stale_locks(&static_backup_path);
    }

    // 2. Remove the symlink bridge
    if let Ok(metadata) = fs::symlink_metadata(&full_profile_path) {
        if metadata.file_type().is_symlink() {
            println!("Removing symlink bridge for {}...", config.name);
            let _ = fs::remove_file(&full_profile_path);
        }
    }

    // 3. Rename backup folder back to original profile directory path
    if static_backup_path.exists() {
        println!("Restoring backup folder to original path...");
        if let Err(e) = fs::rename(&static_backup_path, &full_profile_path) {
            eprintln!("Error restoring backup folder for {}: {:?}", config.name, e);
        } else {
            cleanup_stale_locks(&full_profile_path);
            sanitize_chromium_preferences(&full_profile_path);
            println!("Successfully restored {} profile to SSD.", config.name);
        }
    }

    // 4. Clean up volatile folder in /dev/shm
    if volatile_path.exists() {
        println!("Cleaning up RAM directory for {}...", config.name);
        let _ = fs::remove_dir_all(&volatile_path);
    }
}

fn main() {
    let home = env::var("HOME").expect("Could not find HOME variable");
    let home_path = PathBuf::from(home);

    let mut browsers: Vec<BrowserConfig> = Vec::new();

    // 1. Firefox Setup
    let mut firefox_base = home_path.clone();
    firefox_base.push(".config/mozilla/firefox");
    let firefox_ini_path = firefox_base.join("profiles.ini");
    if let Ok(ini_content) = fs::read_to_string(&firefox_ini_path) {
        if let Some(profile_dir) = find_firefox_default_profile(&ini_content) {
            browsers.push(BrowserConfig {
                name: "Firefox",
                base_dir: firefox_base,
                profile_dir_name: profile_dir.to_string(),
                lock_file_name: "lock",
                lock_format: LockFormat::Firefox,
                exclude_patterns: &["cache2", "startupCache", "jumpListCache", "lock", ".parentlock"],
            });
        }
    }

    // 2. Brave Setup (Targeting Brave-Origin-Beta explicitly)
    let mut brave_base = home_path.clone();
    brave_base.push(".config/BraveSoftware/Brave-Origin-Beta");
    if brave_base.exists() {
        browsers.push(BrowserConfig {
            name: "Brave-Origin-Beta",
            base_dir: brave_base,
            profile_dir_name: "Default".to_string(),
            lock_file_name: "SingletonLock",
            lock_format: LockFormat::Chromium,
            exclude_patterns: &[
                "Cache",
                "Code Cache",
                "GPUCache",
                "ShaderCache",
                "SingletonLock",
                "SingletonCookie",
                "SingletonSocket",
                "lockfile",
            ],
        });
    }

    // 3. Google Chrome Setup (Flatpak installation)
    let mut chrome_base = home_path.clone();
    chrome_base.push(".var/app/com.google.Chrome/config/google-chrome");
    if chrome_base.exists() {
        browsers.push(BrowserConfig {
            name: "Chrome-Flatpak",
            base_dir: chrome_base,
            profile_dir_name: "Default".to_string(),
            lock_file_name: "SingletonLock",
            lock_format: LockFormat::Chromium,
            exclude_patterns: &[
                "Cache",
                "Code Cache",
                "GPUCache",
                "ShaderCache",
                "SingletonLock",
                "SingletonCookie",
                "SingletonSocket",
                "lockfile",
            ],
        });
    }

    if browsers.is_empty() {
        println!("No browsers detected. Exiting.");
        return;
    }

    // Initialize all detected browsers (Phase 1, 2, and 3)
    for browser in &browsers {
        process_browser(browser);
    }

    // --- PHASE 5 INITIALIZATION: SIGNAL HANDLER ---
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = running.clone();
    if let Err(e) = ctrlc::set_handler(move || {
        println!("\nTermination signal received. Shutting down gracefully...");
        r.store(false, std::sync::atomic::Ordering::SeqCst);
    }) {
        eprintln!("Error setting signal handler: {:?}", e);
    }

    println!("\nDaemon started! Entering Phase 4 Sync Loop (syncing every hour)...");
    
    // Sync interval: 1 hour (3600 seconds)
    let sync_interval_secs = 3600;
    let mut seconds_counter = 0;

    // --- PHASE 4: SYNC LOOP ---
    while running.load(std::sync::atomic::Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_secs(1));
        seconds_counter += 1;

        if seconds_counter >= sync_interval_secs {
            seconds_counter = 0;
            println!("\n--- Hourly Sync Triggered ---");
            for browser in &browsers {
                sync_volatile_to_backup(browser);
            }
        }
    }

    // --- PHASE 5: GRACEFUL SHUTDOWN ---
    println!("\nInitiating Phase 5: Graceful Shutdown...");
    for browser in &browsers {
        restore_profile_to_disk(browser);
    }
    
    println!("Daemon shutdown complete. Goodbye!");
}
