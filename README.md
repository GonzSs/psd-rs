# PROFILE SYNC DAEMON RS (`psd-rs`)

A lightweight, zero-bloat system daemon written in pure modern Rust that syncs browser and application profiles to volatile memory (`/dev/shm` RAM) to eliminate disk I/O, speed up responsiveness, and extend SSD lifespan.

Inspired by [profile-sync-daemon](https://wiki.archlinux.org/title/Profile-sync-daemon), re-engineered for minimal overhead, absolute reliability, and zero runtime dependencies outside standard Unix utilities.

---

## What's New in v0.2.0

- **Separation of Mechanism from Policy:** No hardcoded browsers or detection heuristics. The daemon is completely driven by a plain-text configuration file (`~/.config/psd-rs/config`) using an `ssh_config`-style syntax.
- **Engine-Aware Profiles:**
  - `chromium`: SingletonLock/Socket verification, session cookie preservation, Preferences crash-flag healing.
  - `gecko`: Firefox `profiles.ini` automated discovery (`ProfileDir auto`), cross-distro path fallback, `.parentlock` handling.
  - `generic`: Safe RAM relocation and synchronization for any application (e.g., Tauri/WebKit apps like `whatRust`, Electron apps, or local databases).
- **Zero-Bloat Logging & Alerts:** Custom macro-based logging (`error`, `info`, `debug`), terminal bell alerting on critical failures, and persistent error state tracking via `$XDG_RUNTIME_DIR/psd-rs.error`.
- **Modular Architecture:** Clean library and orchestrator split (`config`, `browser`, `recovery`, `sync`, `logging`, `lib`).

---

## Features

- **RAM Execution:** Relocates profiles into `/dev/shm` tmpfs and bridges them with atomic symbolic links.
- **Multi-User Isolation:** Generates user-namespaced paths in RAM (e.g. `/dev/shm/$USER-brave-Default`) to safely support concurrent users on the same machine.
- **Self-Healing & Crash Recovery:**
  - Automatically heals dangling symlinks after system crashes, power loss, or ungraceful reboots.
  - Resolves split-brain directory conflicts by archiving unstaged folders with `-stale` suffixes rather than panicking or losing data.
  - Cooldown settling delay to let SQLite WAL and filesystem caches flush before initiating synchronization.
- **Graceful Shutdown:** Traps termination signals (`SIGTERM`/`SIGINT`), waits for browser processes to exit within a configurable grace period, executes a final differential sync, unlinks the RAM directory, and restores physical directories to disk.

---

## Configuration

`psd-rs` reads its configuration from `~/.config/psd-rs/config` (or `$XDG_CONFIG_HOME/psd-rs/config`).

A template configuration is provided in [`config.example`](config.example).

### Example Configuration

```ini
# Global Settings
SyncInterval 3600
CooldownDelay 1500
ShutdownGracePeriod 3000
VolatilePath /dev/shm
LogLevel info

# Firefox (Gecko)
Browser firefox
    BaseDir ~/.config/mozilla/firefox
    ProfileDir auto
    Type gecko

# Brave (Chromium)
Browser brave
    BaseDir ~/.config/BraveSoftware/Brave-Origin-Beta
    ProfileDir Default
    Type chromium

# WhatsApp Desktop / Tauri app (Generic)
Browser whatrust
    BaseDir ~/.local/share
    ProfileDir com.karem.whatrust
    Type generic
```

### Config Options

| Keyword | Description | Default |
|---|---|---|
| `SyncInterval` | Seconds between periodic RAM-to-disk background syncs | `3600` (1 hour) |
| `CooldownDelay` | Milliseconds to wait for filesystem lock settlement | `1500` |
| `ShutdownGracePeriod` | Milliseconds to wait for browser processes to exit on shutdown | `3000` |
| `VolatilePath` | Base path for volatile RAM storage | `/dev/shm` |
| `LogLevel` | Output verbosity: `error`, `info`, or `debug` | `info` |
| `Type` | Profile engine: `chromium`, `gecko`, or `generic` | *Required* |
| `BaseDir` | Directory containing the profile folder (`~` expanded) | *Required* |
| `ProfileDir` | Profile folder name (or `auto` for `gecko`) | *Required* |
| `LockFile` | Lock file name to check for process liveness | Engine default |
| `Exclude` | Space-separated list of rsync exclude patterns (quote paths with spaces) | Engine default |

---

## Installation

### Option A: Install from Release Binary

Download the release archive from GitHub:

```bash
# 1. Download release and signature
curl -fsSLO https://github.com/GonzSs/psd-rs/releases/latest/download/psd-rs.tar.gz
curl -fsSLO https://github.com/GonzSs/psd-rs/releases/latest/download/psd-rs.tar.gz.asc

# 2. (Optional) Verify PGP signature
gpg --verify psd-rs.tar.gz.asc psd-rs.tar.gz

# 3. Extract and install
tar -xzf psd-rs.tar.gz
install -Dm 755 psd-rs ~/.local/bin/psd-rs

# 4. Initialize configuration
mkdir -p ~/.config/psd-rs
cp config.example ~/.config/psd-rs/config
```

### Option B: Build from Source

```bash
git clone https://github.com/GonzSs/psd-rs.git
cd psd-rs

# Build optimized release binary
cargo build --release

# Install binary
install -Dm 755 target/release/psd-rs ~/.local/bin/psd-rs

# Set up configuration
mkdir -p ~/.config/psd-rs
cp config.example ~/.config/psd-rs/config
```

---

## Service Supervision

### Systemd (User Service)

1. Create the systemd user service file at `~/.config/systemd/user/psd-rs.service`:

```ini
[Unit]
Description=Profile Sync Daemon (Rust)
After=local-fs.target

[Service]
Type=simple
ExecStart=%h/.local/bin/psd-rs
Restart=on-failure
RestartSec=5
TimeoutStopSec=120

[Install]
WantedBy=default.target
```

2. Enable and start:

```bash
systemctl --user daemon-reload
systemctl --user enable --now psd-rs.service
```

### Runit (User Service)

1. Create service directories:

```bash
mkdir -p ~/.config/runit/sv/psd-rs/log
mkdir -p ~/.config/runit/service
```

2. Install run and log scripts:

```bash
cp runit/run ~/.config/runit/sv/psd-rs/run
cp runit/log/run ~/.config/runit/sv/psd-rs/log/run
chmod +x ~/.config/runit/sv/psd-rs/run ~/.config/runit/sv/psd-rs/log/run
```

3. Enable the service:

```bash
ln -s ~/.config/runit/sv/psd-rs ~/.config/runit/service/psd-rs
```

---

## License

[MIT](LICENSE)
