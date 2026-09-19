// config.rs — ssh_config-style configuration parser.
//
// Reads ~/.config/psd-rs/config (or $XDG_CONFIG_HOME/psd-rs/config).
// Format: keyword-value pairs separated by whitespace, # comments,
// "Browser <name>" blocks with indented options.

use std::env;
use std::fs;
use std::path::PathBuf;

use crate::logging::LogLevel;
use crate::{BrowserConfig, BrowserType};

/// Global configuration for the psd-rs daemon.
pub struct PsdConfig {
    pub sync_interval: u64,
    pub cooldown_delay: u64,
    pub shutdown_grace_period: u64,
    pub volatile_path: PathBuf,
    pub log_level: LogLevel,
    pub browsers: Vec<BrowserConfig>,
}

impl Default for PsdConfig {
    fn default() -> Self {
        PsdConfig {
            sync_interval: 3600,
            cooldown_delay: 1500,
            shutdown_grace_period: 3000,
            volatile_path: PathBuf::from("/dev/shm"),
            log_level: LogLevel::Info,
            browsers: Vec::new(),
        }
    }
}

/// Expand a leading ~ to $HOME in a path string.
/// Only expands ~ at the start of the path, matching ssh_config behavior.
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    } else if path == "~" {
        if let Ok(home) = env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(path)
}

/// Parse a space-separated value list, respecting double-quoted strings.
///
/// Example: `Cache "Code Cache" GPUCache` → ["Cache", "Code Cache", "GPUCache"]
fn parse_space_separated(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for c in s.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            ' ' if !in_quotes => {
                if !current.is_empty() {
                    result.push(current.clone());
                    current.clear();
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

/// Parses Firefox's profiles.ini file and extracts the default profile path.
///
/// First checks [Install...] sections for a Default key (the canonical method),
/// then falls back to [Profile...] sections with Default=1.
pub fn find_firefox_default_profile(ini_content: &str) -> Option<String> {
    // First pass: look for [Install...] section with Default key
    let mut in_install_section = false;
    for line in ini_content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            let section = &line[1..line.len() - 1];
            in_install_section =
                section.len() >= 7 && section[..7].eq_ignore_ascii_case("install");
        } else if in_install_section {
            if let Some(eq_idx) = line.find('=') {
                let key = line[..eq_idx].trim();
                let val = line[eq_idx + 1..].trim();
                if key.eq_ignore_ascii_case("default") {
                    return Some(val.to_string());
                }
            }
        }
    }

    // Fallback: check [ProfileX] sections
    let mut current_profile_path: Option<&str> = None;
    let mut is_default_profile = false;

    for line in ini_content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            if is_default_profile && current_profile_path.is_some() {
                return current_profile_path.map(|s| s.to_string());
            }
            current_profile_path = None;
            is_default_profile = false;
        } else if let Some(eq_idx) = line.find('=') {
            let key = line[..eq_idx].trim();
            let val = line[eq_idx + 1..].trim();
            if key.eq_ignore_ascii_case("path") {
                current_profile_path = Some(val);
            } else if key.eq_ignore_ascii_case("default") && val == "1" {
                is_default_profile = true;
            }
        }
    }

    if is_default_profile && current_profile_path.is_some() {
        return current_profile_path.map(|s| s.to_string());
    }

    current_profile_path.map(|s| s.to_string())
}

/// Load the psd-rs configuration from the config file.
///
/// Searches for the config file at $XDG_CONFIG_HOME/psd-rs/config,
/// falling back to ~/.config/psd-rs/config.
pub fn load() -> Result<PsdConfig, String> {
    let config_dir = env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".config")
        });
    let config_path = config_dir.join("psd-rs").join("config");

    if !config_path.exists() {
        return Err(format!(
            "Configuration file not found: {}\n\
             Create it with browser definitions. See the config.example file for format.",
            config_path.display()
        ));
    }

    let content = fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config file {}: {}", config_path.display(), e))?;

    let mut config = parse(&content)?;

    // Resolve "auto" ProfileDir for gecko browsers
    resolve_auto_profiles(&mut config)?;

    Ok(config)
}

/// Parse config file content into a PsdConfig.
fn parse(content: &str) -> Result<PsdConfig, String> {
    let mut config = PsdConfig::default();
    let mut current_browser: Option<BrowserBuilder> = None;

    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Skip comments and empty lines
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Indented lines belong to the current Browser block
        let is_indented = line.starts_with(' ') || line.starts_with('\t');

        if is_indented {
            let browser = current_browser.as_mut().ok_or_else(|| {
                format!(
                    "Line {}: indented option '{}' found outside a Browser block",
                    line_num + 1,
                    trimmed
                )
            })?;
            parse_browser_option(browser, trimmed, line_num + 1)?;
        } else if let Some(name) = trimmed.strip_prefix("Browser ") {
            // Finalize the previous browser block, if any
            if let Some(builder) = current_browser.take() {
                config.browsers.push(builder.build()?);
            }
            current_browser = Some(BrowserBuilder::new(name.trim().to_string()));
        } else {
            // Global option
            parse_global_option(&mut config, trimmed, line_num + 1)?;
        }
    }

    // Finalize the last browser block
    if let Some(builder) = current_browser.take() {
        config.browsers.push(builder.build()?);
    }

    Ok(config)
}

/// Resolve ProfileDir "auto" for gecko browsers by reading profiles.ini.
fn resolve_auto_profiles(config: &mut PsdConfig) -> Result<(), String> {
    for browser in &mut config.browsers {
        if browser.profile_dir_name == "auto" {
            match browser.browser_type {
                BrowserType::Gecko => {
                    let ini_path = browser.base_dir.join("profiles.ini");
                    let ini_content = fs::read_to_string(&ini_path).map_err(|e| {
                        format!(
                            "Browser '{}': ProfileDir is 'auto' but cannot read {}: {}",
                            browser.name,
                            ini_path.display(),
                            e
                        )
                    })?;
                    let profile_dir =
                        find_firefox_default_profile(&ini_content).ok_or_else(|| {
                            format!(
                                "Browser '{}': ProfileDir is 'auto' but no default profile found in {}",
                                browser.name,
                                ini_path.display()
                            )
                        })?;
                    browser.profile_dir_name = profile_dir;
                }
                _ => {
                    return Err(format!(
                        "Browser '{}': ProfileDir 'auto' is only valid for Type gecko",
                        browser.name
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Parse a global (non-indented) option line.
fn parse_global_option(config: &mut PsdConfig, line: &str, line_num: usize) -> Result<(), String> {
    let (key, value) = split_keyword_value(line, line_num)?;
    match key.to_lowercase().as_str() {
        "syncinterval" => {
            config.sync_interval = value
                .parse()
                .map_err(|_| format!("Line {}: invalid SyncInterval value: '{}'", line_num, value))?;
        }
        "cooldowndelay" => {
            config.cooldown_delay = value
                .parse()
                .map_err(|_| format!("Line {}: invalid CooldownDelay value: '{}'", line_num, value))?;
        }
        "shutdowngraceperiod" => {
            config.shutdown_grace_period = value.parse().map_err(|_| {
                format!(
                    "Line {}: invalid ShutdownGracePeriod value: '{}'",
                    line_num, value
                )
            })?;
        }
        "volatilepath" => {
            config.volatile_path = PathBuf::from(value);
        }
        "loglevel" => {
            config.log_level = LogLevel::from_str(value).ok_or_else(|| {
                format!(
                    "Line {}: invalid LogLevel '{}' (expected error, info, or debug)",
                    line_num, value
                )
            })?;
        }
        _ => {
            eprintln!(
                "[WARNING] Line {}: unknown global option '{}', ignoring",
                line_num, key
            );
        }
    }
    Ok(())
}

/// Builder for constructing a BrowserConfig from parsed config lines.
struct BrowserBuilder {
    name: String,
    base_dir: Option<PathBuf>,
    profile_dir: Option<String>,
    lock_file: Option<String>,
    browser_type: Option<BrowserType>,
    exclude: Vec<String>,
}

impl BrowserBuilder {
    fn new(name: String) -> Self {
        BrowserBuilder {
            name,
            base_dir: None,
            profile_dir: None,
            lock_file: None,
            browser_type: None,
            exclude: Vec::new(),
        }
    }

    /// Build the final BrowserConfig, applying type-based defaults for
    /// LockFile and Exclude if not explicitly set.
    fn build(self) -> Result<BrowserConfig, String> {
        let browser_type = self.browser_type.ok_or_else(|| {
            format!("Browser '{}': missing required 'Type' option", self.name)
        })?;

        let base_dir = self.base_dir.ok_or_else(|| {
            format!("Browser '{}': missing required 'BaseDir' option", self.name)
        })?;

        let profile_dir = self.profile_dir.ok_or_else(|| {
            format!(
                "Browser '{}': missing required 'ProfileDir' option",
                self.name
            )
        })?;

        // Default lock file based on browser type
        let lock_file = self.lock_file.unwrap_or_else(|| match browser_type {
            BrowserType::Chromium => "SingletonLock".to_string(),
            BrowserType::Gecko => "lock".to_string(),
            BrowserType::Generic => "lockfile".to_string(),
        });

        // Default exclude patterns based on browser type
        let exclude = if self.exclude.is_empty() {
            match browser_type {
                BrowserType::Chromium => vec![
                    "Cache".to_string(),
                    "Code Cache".to_string(),
                    "GPUCache".to_string(),
                    "ShaderCache".to_string(),
                    "SingletonLock".to_string(),
                    "SingletonSocket".to_string(),
                    "lockfile".to_string(),
                ],
                BrowserType::Gecko => vec![
                    "cache2".to_string(),
                    "startupCache".to_string(),
                    "jumpListCache".to_string(),
                    "lock".to_string(),
                    ".parentlock".to_string(),
                ],
                BrowserType::Generic => Vec::new(),
            }
        } else {
            self.exclude
        };

        Ok(BrowserConfig {
            name: self.name,
            base_dir,
            profile_dir_name: profile_dir,
            lock_file_name: lock_file,
            exclude_patterns: exclude,
            browser_type,
        })
    }
}

/// Parse an indented option line within a Browser block.
fn parse_browser_option(
    builder: &mut BrowserBuilder,
    line: &str,
    line_num: usize,
) -> Result<(), String> {
    let (key, value) = split_keyword_value(line, line_num)?;
    match key.to_lowercase().as_str() {
        "basedir" => {
            builder.base_dir = Some(expand_tilde(value));
        }
        "profiledir" => {
            builder.profile_dir = Some(value.to_string());
        }
        "lockfile" => {
            builder.lock_file = Some(value.to_string());
        }
        "type" => {
            builder.browser_type = Some(match value.to_lowercase().as_str() {
                "chromium" => BrowserType::Chromium,
                "gecko" => BrowserType::Gecko,
                "generic" => BrowserType::Generic,
                _ => {
                    return Err(format!(
                        "Browser '{}': unknown Type '{}' (expected chromium, gecko, or generic)",
                        builder.name, value
                    ))
                }
            });
        }
        "exclude" => {
            builder.exclude = parse_space_separated(value);
        }
        _ => {
            eprintln!(
                "[WARNING] Browser '{}', line {}: unknown option '{}', ignoring",
                builder.name, line_num, key
            );
        }
    }
    Ok(())
}

/// Split a line into keyword and value, separated by whitespace.
///
/// "SyncInterval 3600" → ("SyncInterval", "3600")
fn split_keyword_value(line: &str, line_num: usize) -> Result<(&str, &str), String> {
    let line = line.trim();
    if let Some(idx) = line.find(|c: char| c.is_whitespace()) {
        let key = &line[..idx];
        let value = line[idx..].trim_start();
        if value.is_empty() {
            Err(format!("Line {}: option '{}' has no value", line_num, key))
        } else {
            Ok((key, value))
        }
    } else {
        Err(format!(
            "Line {}: invalid option (no value): '{}'",
            line_num, line
        ))
    }
}
