
# PROFILE SYNC DAEMON RS

## psd-rs

A lightweight, zero-bloat system daemon written in Rust that syncs browser
profiles to volatile memory (`/dev/shm` RAM) to reduce disk I/O, speed up
browser responsiveness, and extend SSD lifespan.

Inspired by [profile-sync-daemon](https://wiki.archlinux.org/title/Profile-sync-daemon).

Written with AI assistance (Antigravity CLI) under my direction.
Specifically I directed it to look into:

- Reducing CPU and RAM usage.
- As I am still learning Rust I used this to understand Stack vs Heap Memory.
- Focused on edge cases such as handling reboots, multi-user use case and lockouts.

## Features

- **Supported Browsers:** `brave-origin-beta`, Firefox, and `flatpak` Google Chrome.
- **RAM Execution:** Symlinks active profiles to `/dev/shm` for
low-latency memory operations.
- **Hourly Backup Sync:** Syncs RAM profile data back to physical disk storage.
- **Self-Healing & Crash Recovery:**
  - Auto-recovers dangling symlinks after system crashes or ungraceful shutdowns.
  - Purges stale locks:
  (`SingletonLock`, `SingletonCookie`, `SingletonSocket`, `lockfile`, `lock`, `.parentlock`).
  - Sanitizes Chromium `Preferences` (`exit_type: Normal`),
  preventing "Closed unexpectedly" banners and session cookie invalidation.
- **Graceful Shutdown:** Intercepts `SIGTERM`/`SIGINT` signals and waits
for browser process termination before restoring profiles back to physical disk.

## Build & Installation

```bash
git clone https://github.com/GonzSs/psd-rs.git
cd psd-rs
cargo build --release
cp target/release/psd-rs ~/.local/bin/
```

## Usage

### Running Directly

You can start the daemon manually in the foreground:

```bash
psd-rs
```

### Running as a Runit User Service

To set up `psd-rs` to run automatically and log gracefully under `runit` in user-space:

1. **Create the target directory structure** in your home directory:

   ```bash
   mkdir -p ~/.config/runit/sv/psd-rs/log
   mkdir -p ~/.config/runit/service
   ```

2. **Copy the service templates** from the repository to your config folder:

   ```bash
   cp runit/run ~/.config/runit/sv/psd-rs/run
   cp runit/log/run ~/.config/runit/sv/psd-rs/log/run
   ```

3. **Make the run scripts executable**:

   ```bash
   chmod +x ~/.config/runit/sv/psd-rs/run
   chmod +x ~/.config/runit/sv/psd-rs/log/run
   ```

4. **Enable the service** by creating a symbolic link in the active services folder:

   ```bash
   ln -s ~/.config/runit/sv/psd-rs ~/.config/runit/service/psd-rs
   ```

5. **Configure autostart**:
   Add this line to your display manager autostart file (like `~/.xprofile`),
   or your shell profile so that the supervisor starts upon login:
  
   ```bash
   runsvdir -P ~/.config/runit/service &
   ```

## License

[MIT](LICENSE)
