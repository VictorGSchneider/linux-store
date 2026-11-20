#!/bin/sh
# Installs the privileged helper and its polkit policy on THIS machine, for development.
#
# This is the only step that puts root-owned files on the system. Run it yourself:
#   cargo build --release -p store-helper -p store-cli     (as your normal user, first)
#   sudo sh packaging/dev-install.sh
# Undo it with:  sudo sh packaging/dev-uninstall.sh
#
# The helper must be a --release build (see AGENTS.md: the alpm crate trips an alignment check
# in debug builds).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
helper="$root/target/release/store-helper"
policy="$root/packaging/org.linuxstore.helper.policy"

if [ "$(id -u)" -ne 0 ]; then
    echo "run this with sudo: sudo sh packaging/dev-install.sh" >&2
    exit 1
fi
if [ ! -x "$helper" ]; then
    echo "build the helper first, as your normal user: cargo build --release -p store-helper" >&2
    exit 1
fi

install -D -m 0755 -o root -g root "$helper" /usr/lib/linux-store/store-helper
install -D -m 0644 -o root -g root "$policy" /usr/share/polkit-1/actions/org.linuxstore.helper.policy
echo "installed:"
echo "  /usr/lib/linux-store/store-helper"
echo "  /usr/share/polkit-1/actions/org.linuxstore.helper.policy"
echo "next, as your normal user: sh packaging/manual-check.sh"
