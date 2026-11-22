# Agent Instructions

linux-store is a universal Linux app store written in Rust, with a
[Slint](https://slint.dev) UI. The design is in `docs/2026-11-18-linux-store-design.md`.
This file may be older than the code. Where they disagree, trust the code, and update this file.

## Layout

Cargo workspace: `core/` (package `store-core`; not named `core`, which would collide with Rust's
`core`), `backends/pacman/`, `backends/aur/` and `backends/flatpak/` (packages
`store-backend-pacman`, `-aur`, `-flatpak`), `cli/` (package `store-cli`, binary
`store`), `protocol/` (`store-protocol`), `helper/` (`store-helper`, the only code that runs as
root), `installers/pkexec/` (`store-installer-pkexec`), `packaging/`, `appstream/`
(`store-appstream`, AppStream parsing; the app/package matching is `rebuild_links_in` in
`core/src/index.rs`), `system/` (`store-system`: the
real backends and installer, shared by CLI and GUI), `app/` (`store-app`: the `Engine`, which owns
the index and the installer and runs everything slow on its own threads: query, refresh, operations
and two icon decoders; the UI sends `Command`s and gets `Event`s) and `gui/` (package
`linux-store`: a library, `linux_store`, with `convert`, `state` and `wiring`; the binary; the
`examples/` and the `.slint` files in `gui/ui/`).
Planned: more `backends/*`. GUI design and plan: `docs/2026-11-21-gui-design.md` and
`docs/2026-11-21-gui-round1-plan.md` (round 1 is done; categories, sidebar, settings and the
Flatpak/AUR installers are round 2+).
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

## Rules for agents working on the GUI

- Never run the GUI with the default renderer (see the freeze below), and never start an install,
  remove or upgrade from it. Look at the UI through the headless examples in `gui/examples/`
  (each doc comment has its command, e.g. `SLINT_BACKEND=headless cargo run -p linux-store
  --example fake_ops --features slint/mcp -- OUT_DIR`), which write PNGs you can Read.
- Examples and tests use fakes only (`store_core::testing`: `FakeBackend`, `FakeInstaller`) and
  NEVER construct the real installer. Only `gui/src/main.rs` builds it.
- A real-engine run (`live_snapshot`) must use a temporary `XDG_CACHE_HOME`; the first refresh
  downloads the AUR dump. Its snapshots show this machine's installed packages: do not commit them.
  The README's `home.png` and `app-page.png` come from `readme_shots` (real catalogs, no installed
  state), `installeds.png` from `fake_lists` (cropped to 1000x440). Never edit their pixels.
- Arch's AppStream icons are JPEG XL (`.jxl`), decoded with `jxl-oxide` in `app/src/icons.rs`
  (PNG too); an icon that fails to decode falls back to a letter tile.
- Size contracts are in the headers of `gui/ui/row.slint`, `home-page.slint` and `list-page.slint`
  (rows and home are not virtualised; the lists instantiate only the visible cards, because
  positions past ~32000 px break the software renderer). Read them before changing those pages.
- Manual verification with the real helper is `packaging/manual-gui-check.md`, run by the user only.

## Loops and continuous documentation

Work is split into packages (P0, P1, ... in `docs/log/progress.json`); each plan task is a step.
Three loops, kept separate so the agent's context stays small:

1. **Execution loop.** Take the next unchecked task of the current plan, do it (TDD), commit, move on.
   Do not stop between tasks unless something needs the user.
2. **Monitoring loop.** For builds, tests and CI that take a while, start them with
   `scripts/watch-build.sh start <dir> <command...>` and poll with `scripts/watch-build.sh wait <dir> 60`
   (or `/loop 1m`). Each check reads only the exit code (`ok` or `falhou <code>`), never the compiler
   output. Do not read `out.log` while it is running or when it passed.
3. **Fix loop.** Only on failure: `scripts/log-excerpt.sh <dir>/out.log` prints just the first error and
   its context. Fix that error, add an `E-NNN` entry to `docs/log/errors.md` (symptom, cause, fix, commit),
   run again through the monitoring loop. After 3 failed tries on the same error, stop and ask the user.

Documentation is updated as part of every package, not at the end (all in Portuguese, like the designs):

- `docs/log/journal.md`: chronological diary, one line per fact, with the commit hash.
- `docs/log/errors.md`: error registry (`E-NNN`: symptom, cause, fix).
- `docs/log/decisions.md`: technical decisions (`D-NNN`: context, decision, reason, alternatives).
- `docs/log/progress.json` is the data source. After closing a package, update it (done/total, status,
  tests) and run `python3 -I scripts/gen-dashboard.py`: it rewrites `STATUS.md` and
  `docs/log/dashboard.html`. Never edit those two by hand.
- Commit the log files in the same commit as the work, or in a `docs: log P<N>` commit right after.

## Token budget

Spend as few tokens as possible; every rule below is about not reading or writing more than needed.

- Read only what the task needs: `grep`/`rg` first, then `Read` with `offset`/`limit`. Never read
  `Cargo.lock`, `target/`, images or `legacy/` (denied in `.claude/settings.json`), and do not re-read
  a file you just edited.
- Start from `STATUS.md` and the last entries of `docs/log/journal.md`, not from the whole `docs/`.
- Run cargo quietly and capture the output: `cargo -q ... >out.log 2>&1`, then show only
  `scripts/log-excerpt.sh out.log`. Use `cargo test -q` and `cargo check` before a full build.
  Never paste a full build or test log into the conversation (see the monitoring and fix loops).
- Do not spawn subagents for work you can do inline in a few calls. Subagents run on Sonnet
  (`CLAUDE_CODE_SUBAGENT_MODEL`); give them a precise, self-contained prompt and ask for a report of at
  most 15 lines (what changed, test result, open problems).
- Batch independent tool calls in one turn; do not re-run a command whose answer you already have.
- Answers to the user: short, in Portuguese, no recap of what the diff already shows. Commit messages
  and PR bodies state what and why in a few lines.
- Do not load skills or plugins that the task does not need.

## Install the Slint Plugin

Before you work on the UI, install the Slint plugin from [slint-ui/ai-plugins](https://github.com/slint-ui/ai-plugins).
It teaches you the `.slint` language and its pitfalls.
It also lets you search the documentation, and lets you render and drive the running UI.
(Some agent sessions already have it; check before suggesting the install. Driving the running
app is subject to the renderer and no-install rules above and below.)
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

CI (`.github/workflows/ci.yml`) runs rustfmt, clippy with `-D warnings`, the tests, and for the GUI
a compile check of all targets (examples included) and its unit tests.
Run the same locally: `cargo fmt --all --check`, `cargo clippy --workspace --exclude linux-store --all-targets --features store-core/testing -- -D warnings`,
`cargo test --workspace --exclude linux-store --features store-core/testing`,
`cargo check --locked -p linux-store --all-targets`, `cargo test --locked -p linux-store`.

## Documentation

Slint docs: https://docs.slint.dev/latest/docs/slint/ — consult it (or the plugin's docs search) for Slint
language and API questions instead of relying on memory.

## Build and Run

```sh
cargo build
cargo run
```

On the main dev laptop `cargo run` of the GUI must use one of the safe renderer settings from the
Known issue section below; agents never run it (they use the headless examples).

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

While you work on the UI, build and run the application with live preview
(on the owner's machine, with the safe renderer settings above; agents use the headless examples instead).
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
  use the plugin's preview, or a screenshot of the live-preview application (owner's machine);
  agents Read the PNGs the headless examples in `gui/examples/` write.
- When the UI changes visibly, update the screenshots in `README.md`.
- Format changed `.slint` files with `slint-lsp format -i <file>`.
- If `slint-viewer`/`slint-lsp` are not installed (as in the agent sandbox so far),
  `cargo check -p linux-store --all-targets` is the compile check and the headless examples are
  the way to look at the UI.
