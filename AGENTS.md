# Agent Instructions

linux-store is a universal Linux app store written in Rust, with a
[Slint](https://slint.dev) UI. The design is in `docs/2026-11-18-linux-store-design.md`.
This file may be older than the code. Where they disagree, trust the code, and update this file.

## Layout

Cargo workspace: `core/` (package `store-core`; not named `core`, which would collide with Rust's
`core`), `backends/pacman/`, `backends/aur/` and `backends/flatpak/` (packages
`store-backend-pacman`, `-aur`, `-flatpak`), `cli/` (package `store-cli`, binary
`store`), `protocol/` (`store-protocol`), `helper/` (`store-helper`, the only code that runs as
root), `installers/pkexec/` (`store-installer-pkexec`), `packaging/`, `app/` (`store-app`, the GUI engine), `system/` and `gui/` (the Slint app: `lib.rs` with `convert` and `wiring`, thin `main.rs`).
Planned: more `backends/*`.
Run the core's tests with `cargo test --workspace --features store-core/testing`.

## The privileged helper: never run it from an agent

`store-helper`, `pkexec`, `sudo` and `store install|remove|upgrade` change the real system or
raise password prompts. Agents must not run them. The helper's tests use a temporary root and a
`file://` repository (they need `bsdtar`, `zstd` and `repo-add`); the only privileged check is
`packaging/manual-check.sh`, which the user runs after `sudo sh packaging/dev-install.sh`.
Build the helper with `--release`: the `alpm` crate trips an alignment check in debug builds
(see `[profile.dev.package.alpm]` in the root `Cargo.toml`). The test-only paths (`--root`,
`--dbpath`) exist only under `#[cfg(test)]` in `helper/`; never add a flag, feature or
environment variable that exposes them.
`legacy/` holds the old bash `stoa-store`; use it as reference only.

## Install the Slint Plugin

Before you work on the UI, install the Slint plugin from [slint-ui/ai-plugins](https://github.com/slint-ui/ai-plugins).
It teaches you the `.slint` language and its pitfalls.
It also lets you search the documentation, and lets you render and drive the running UI.
If it isn't installed, suggest that the user install it:

```sh
# Claude Code
/plugin marketplace add slint-ui/ai-plugins
/plugin install slint@slint

# Codex
codex plugin marketplace add slint-ui/ai-plugins
codex plugin add slint@slint

# Other agents
npx skills add slint-ui/ai-plugins --skill slint
```

## Before you push

CI (`.github/workflows/ci.yml`) runs rustfmt, clippy with `-D warnings`, the tests and a GUI compile check.
Run the same locally: `cargo fmt --all --check`, `cargo clippy --workspace --exclude linux-store --all-targets --features store-core/testing -- -D warnings`,
`cargo test --workspace --exclude linux-store --features store-core/testing`.

## Documentation

Slint docs: https://docs.slint.dev/latest/docs/slint/ — consult it (or the plugin's docs search) for Slint
language and API questions instead of relying on memory.

## Build and Run

```sh
cargo build
cargo run
```

## Known issue: plain `cargo run` can freeze the dev machine

On the main dev laptop (NVIDIA RTX 4050 + AMD 680M, Wayland/niri, nvidia 620.70.06),
a plain `cargo run` hard-froze the whole system (mouse stuck, forced reboot, nothing in
the journal). Slint's default renderer (femtovg, OpenGL) most likely opened its EGL
context through the NVIDIA driver. Not reproduced on purpose, so treat it as a strong
suspicion. Both of these ran fine:

```sh
# Mesa/AMD EGL only (keeps NVIDIA out of it)
__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json cargo run

# CPU rendering
SLINT_BACKEND=winit-software cargo run
```

Use one of these on that machine until the NVIDIA/EGL problem is understood.

## Live Preview

While you work on the UI, build and run the application with live preview.
It compiles much faster, because the `.slint` files aren't compiled to native code.

```sh
SLINT_LIVE_PREVIEW=1 cargo run --features slint/live-preview
```

Use the same environment variable and feature with `cargo build` and `cargo check`.

The running application reloads the `.slint` files when you save them, with no rebuild or restart.
The state your code set, such as properties and models, stays in place.
Rebuild when you rename a property or callback that the code uses.
Use live preview only during development, and keep it out of the build files.

## Working on the UI

- Check `.slint` changes without a full build with `slint-viewer --check .`.
- Look at a render before you call UI work done:
  use the plugin's preview, or a screenshot of the live-preview application.
- When the UI changes visibly, update the screenshots in `README.md`.
- Format changed `.slint` files with `slint-lsp format -i <file>`.
