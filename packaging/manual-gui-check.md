# Manual GUI check (real helper, real password prompt)

**This changes the real system, and it is for YOU to run. Agents must not run it.** It installs
and removes one tiny package with pacman through the privileged helper and polkit. Like
`packaging/manual-check.sh`, it cannot run in CI.

## Prerequisites

1. `cowsay` must not be installed (`pacman -Q cowsay` should say "not found"); this check removes it.
2. Build the helper in release mode (the `alpm` crate trips an alignment check in debug builds) and
   the GUI, then install the helper and its polkit policy (the only root-owned files):

       cargo build --release -p store-helper
       cargo build -p linux-store
       sudo sh packaging/dev-install.sh

3. Run the GUI with a safe renderer. On a hybrid NVIDIA laptop a plain `cargo run` froze a machine
   (see AGENTS.md). Use either of these from the repository root:

       SLINT_BACKEND=winit-software cargo run -p linux-store
       __EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json cargo run -p linux-store

   Keep that terminal visible: the GUI's stderr (`linux-store: ...`) goes there.

## Checks

1. Wait for the first index refresh (about 15 seconds). Home shows three rows (pacman, Flatpak,
   AUR) and a chip per source with its package count.
2. Search: type `firefox`. Results appear as you type; click the `flatpak` chip to filter, click it
   again to clear.
3. Click a pacman result. The page shows one button per source, in the order of your host; the
   Flatpak and AUR buttons are disabled ("no installer for this source yet"), pacman's is active.
4. Search `cowsay`, open the pacman one and click Install. A polkit dialog asks for your password.
   Enter it. A progress bar appears and the page ends as installed. Then in a terminal:
   `pacman -Q cowsay` must print a version.
5. Remove `cowsay` from its page. After the password (or without one, within about five minutes),
   `pacman -Q cowsay` must say "not found" again.
6. Cancel: install `cowsay` again, enter the password and press Cancel while the bar is moving.
   The GUI must return to idle and say the operation was cancelled. A tiny package can finish before
   you click; if so, remove it (check 5) and retry with a larger one (a few MiB). Afterwards check
   `pacman -Q` for whatever you installed and remove it.
7. Wrong password: install `cowsay` again, type a wrong password (several times if the dialog
   allows) or dismiss the dialog with Escape. The GUI must return to idle and show a message, and
   `pacman -Q cowsay` must say "not found".
8. Installeds: open it, scroll, then Tab into the list and use the arrow keys, PageUp/PageDown, Home and End. Open an app from it and press Back: you return to the
   list. Updates: it lists pacman updates only (it may be empty).
9. Narrow window: resize the window to about 480 px wide. Cards re-flow, nothing is cut off, and
   the app page stays readable.

## If something fails

- Read the terminal that started the GUI: engine and installer errors are printed to stderr.
- Is the helper installed? `ls -l /usr/lib/linux-store/store-helper
  /usr/share/polkit-1/actions/org.linuxstore.helper.policy`.
- An `upgrade_needed` error means the package databases are ahead of the installed system (for
  example after `pacman -Sy`): upgrade the system first, then retry.
- Anything left installed: `sudo pacman -Rs cowsay`.

## Clean up

       sudo sh packaging/dev-uninstall.sh
