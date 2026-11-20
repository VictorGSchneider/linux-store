#!/bin/sh
# The end-to-end check that cannot run in CI: a real password prompt, a real pacman transaction.
# Run it yourself, as your normal user, after packaging/dev-install.sh:
#   cargo build --release -p store-cli
#   sh packaging/manual-check.sh
#
# It installs and removes two tiny packages (tree, cowsay) through the real helper. It refuses to
# start if either is already installed, so it never removes something you use.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
store="$root/target/release/store"

if [ ! -x "$store" ]; then
    echo "build the CLI first: cargo build --release -p store-cli" >&2
    exit 1
fi
if [ ! -x /usr/lib/linux-store/store-helper ] || [ ! -f /usr/share/polkit-1/actions/org.linuxstore.helper.policy ]; then
    echo "the helper is not installed: sudo sh packaging/dev-install.sh" >&2
    exit 1
fi
for package in tree cowsay; do
    if pacman -Q "$package" >/dev/null 2>&1; then
        echo "$package is already installed, and this check would remove it. Aborting." >&2
        exit 1
    fi
done

pause() {
    printf '\n%s\nPress Enter to run it. ' "$1"
    read -r _
}

echo "This will run three real package operations. Watch for the password dialog."
echo "(Optional: in another terminal during step 1, run:  ps -eo pid,ppid,user,args | grep -E 'pkexec|store-helper')"

pause "STEP 1: store install tree        -> a password dialog SHOULD appear."
"$store" install tree

pause "STEP 2: store install cowsay      -> within about 5 minutes of step 1 it should NOT ask again."
"$store" install cowsay

pause "STEP 3: store remove tree cowsay  -> a different action, but the same helper: note whether it asks."
"$store" remove tree cowsay

echo
for package in tree cowsay; do
    if pacman -Q "$package" >/dev/null 2>&1; then
        echo "WARNING: $package is still installed"
    else
        echo "ok: $package is gone"
    fi
done
echo
echo "Please report back:"
echo "  1. Did a graphical password dialog appear in step 1 (not in the terminal)?"
echo "  2. Did step 2 ask for a password again?  Did step 3?"
echo "  3. (optional) In the ps output, was store-helper's parent pkexec, or your shell?"
echo "  4. Any line printed by 'journalctl -t linux-store-helper --since -10min' (it shows what was logged)."
