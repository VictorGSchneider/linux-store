# Agent Instructions

linux-store is a universal Linux app store written in Rust, with a
[Slint](https://slint.dev) UI. The design is in `docs/2026-11-18-linux-store-design.md`.
This file may be older than the code. Where they disagree, trust the code, and update this file.

## Layout

Cargo workspace: `gui/` is the Slint app. Planned crates: `core`, `backends/*`, `helper`.
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

## Build and Run

```sh
cargo build
cargo run
```

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
