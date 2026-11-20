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
#
# The two input files live in a directory your user can write, and the result is run as root, so
# the script does not trust them blindly: it refuses symlinks, reads each file once through a
# no-follow open into a private root-owned directory, checks the COPY (so what is checked is what
# is installed) and installs that copy.
set -eu
umask 022

root=$(cd "$(dirname "$0")/.." && pwd)
helper="$root/target/release/store-helper"
policy="$root/packaging/org.linuxstore.helper.policy"
helper_dest=/usr/lib/linux-store/store-helper
policy_dest=/usr/share/polkit-1/actions/org.linuxstore.helper.policy

refuse() {
    echo "refusing: $1" >&2
    exit 1
}

if [ "$(id -u)" -ne 0 ]; then
    echo "run this with sudo: sudo sh packaging/dev-install.sh" >&2
    exit 1
fi
[ -f "$helper" ] || refuse "build the helper first, as your normal user: cargo build --release -p store-helper"
[ -f "$policy" ] || refuse "$policy is missing"
for input in "$helper" "$policy"; do
    [ ! -L "$input" ] || refuse "$input is a symlink; symlinks are not followed"
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# iflag=nofollow makes the open itself fail on a symlink, so there is no gap between a check and
# the read in which the file could be swapped for a link.
dd if="$helper" of="$tmp/store-helper" iflag=nofollow status=none ||
    refuse "cannot read $helper (is it a symlink?)"
dd if="$policy" of="$tmp/policy" iflag=nofollow status=none ||
    refuse "cannot read $policy (is it a symlink?)"

magic=$(head -c 4 "$tmp/store-helper" | od -An -tx1 | tr -d ' \n')
[ "$magic" = "7f454c46" ] || refuse "$helper is not an ELF executable"
grep -q 'org.linuxstore.helper.run' "$tmp/policy" ||
    refuse "$policy does not define the org.linuxstore.helper.run action"

install -D -m 0755 -o root -g root "$tmp/store-helper" "$helper_dest"
install -D -m 0644 -o root -g root "$tmp/policy" "$policy_dest"
echo "installed:"
echo "  $helper_dest"
echo "  $policy_dest"
echo "next, as your normal user: sh packaging/manual-check.sh"
