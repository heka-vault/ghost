#!/usr/bin/env bash
# Install Ghost's optional GNOME Shell helper for this user (docs/floating-room.md). It is the
# operator's choice: this changes GNOME's list of extensions. On Wayland, log out and back in
# once, then: gnome-extensions enable ghost-desktop@heka.dev
# Remove it with: gnome-extensions uninstall ghost-desktop@heka.dev
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT
gnome-extensions pack --force --out-dir "$out" "$here/ghost-desktop@heka.dev"
gnome-extensions install --force "$out/ghost-desktop@heka.dev.shell-extension.zip"
echo "Installed. Log out and back in, then: gnome-extensions enable ghost-desktop@heka.dev"
