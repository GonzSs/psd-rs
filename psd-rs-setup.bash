#!/usr/bin/env bash

# Exit on command errors (-e), exit on unset variables (-u)
set -eu

echo "psd-rs installing...."

checking_dependencies(){
    if [[ -d /run/systemd/system/ ]]; then
        echo "Found dependency: Systemd is running"
    else
        echo "Couldn't find systemd running. Bye!"
        exit 1
    fi

    if ! command -v rsync > /dev/null 2>&1; then
        echo "command rsync needed. Exiting now"
        exit 1
    fi
}

checking_dependencies

# Create a secure, unique workspace in RAM/disk
WORK_DIR=$(mktemp -d)

# Ensure the work directory is cleaned up automatically on script exit or crash
trap 'rm -rf "$WORK_DIR"' EXIT

# Change to the secure workspace
cd "$WORK_DIR"

# Download the latest release binary.
curl -fsSL -o psd-rs.tar.gz "https://github.com/GonzSs/psd-rs/releases/latest/download/psd-rs.tar.gz"
tar -xzf psd-rs.tar.gz

# Install the binary and user systemd directory
install -Dm 755 psd-rs "$HOME/.local/bin/psd-rs"
install -dm 755 "$HOME/.config/systemd/user"

# Write the systemd user service file
cat << EOF > "$HOME/.config/systemd/user/psd-rs.service"
[Unit]
Description=Profile Sync Daemon written in Rust
After=local-fs.target

[Service]
Type=simple
ExecStart=%h/.local/bin/psd-rs
Restart=on-failure
RestartSec=5
# Give the daemon time to wait for browser flushes during system stop/reboot
TimeoutStopSec=120

[Install]
WantedBy=default.target
EOF

echo "Finishing ..."
sleep 1

# Reload systemd and enable/start the service
systemctl --user daemon-reload
systemctl --user enable --now psd-rs.service
systemctl --user status psd-rs.service

echo "psd-rs successfully installed."
