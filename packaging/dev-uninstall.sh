#!/bin/sh
# Removes what dev-install.sh put on the system.  Run:  sudo sh packaging/dev-uninstall.sh
set -eu

if [ "$(id -u)" -ne 0 ]; then
    echo "run this with sudo: sudo sh packaging/dev-uninstall.sh" >&2
    exit 1
fi
rm -f /usr/lib/linux-store/store-helper /usr/share/polkit-1/actions/org.linuxstore.helper.policy
rmdir /usr/lib/linux-store 2>/dev/null || true
echo "removed the helper and its polkit policy"
