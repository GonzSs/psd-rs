# psd-rs

A lightweight, zero-bloat system daemon written in Rust that syncs browser profiles to volatile memory (`/dev/shm` RAM) to reduce disk I/O, speed up browser responsiveness, and extend SSD lifespan.

Inspired by [profile-sync-daemon](https://wiki.archlinux.org/title/Profile-sync-daemon).

## Features

- **Supported Browsers:** Brave (`Brave-Origin-Beta`), Firefox, and Google Chrome (Flatpak).
- **RAM Execution:** Symlinks active profiles to `/dev/shm` for low-latency memory operations.
- **Hourly Backup Sync:** Periodically syncs volatile RAM profile data back to physical disk storage.
- **Self-Healing & Crash Recovery:**
  - Auto-recovers dangling symlinks after system crashes or ungraceful shutdowns.
  - Purges stale locks (`SingletonLock`, `SingletonCookie`, `SingletonSocket`, `lockfile`, `lock`, `.parentlock`).
  - Sanitizes Chromium `Preferences` (`exit_type: Normal`) to prevent "Closed unexpectedly" banners and session cookie invalidation.
- **Graceful Shutdown:** Intercepts `SIGTERM`/`SIGINT` signals and waits for browser process termination before restoring profiles back to physical disk.

## Build & Installation

```bash
cargo build --release
cp target/release/psd-rs ~/.local/bin/
```

## Usage

Run directly or supervise via `runit` or `systemd`:

```bash
psd-rs
```

## License

[MIT](LICENSE)
