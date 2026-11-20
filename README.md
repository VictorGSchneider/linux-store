# linux-store

[![CI](https://github.com/VictorGSchneider/linux-store/actions/workflows/ci.yml/badge.svg)](https://github.com/VictorGSchneider/linux-store/actions/workflows/ci.yml)

**A universal, open-source app store for Linux.**

One fast, simple graphical app to find and install software from *every* source
on your system: your distro's own repositories, the AUR, Flatpak, Snap, AppImage,
and later drivers, firmware and programming-language toolchains.

Think Microsoft Store, App Store or Play Store, but open, light, and not tied to a
single package format.

> **Status: early development.** The core, the search index and read-only pacman, AUR
> and Flatpak backends work and can be tried from the `store` command line (see below).
> A privileged helper can install, remove and upgrade pacman packages (see below, and note
> that its real-password check has not been done yet); the GUI is still a placeholder window.
> See [Roadmap](#roadmap).

## Why

Linux software lives in many places, and every place has its own tool:

- GNOME Software and KDE Discover cover only part of the picture (no real AUR
  support, nothing for language toolchains, one view per source).
- Tools like bauh cover more sources, but work by driving command-line tools and
  parsing their text output, which is fragile and slow.

linux-store aims to give a regular user **one search box** that works across all of
it, and to stay **small and quick** while doing so.

## Goals

- A simple GUI anyone can use to search, install, update and remove software.
- Fast: instant search, low memory use, a single native binary.
- Many sources behind one interface: distro repos, AUR, Flatpak, Snap, AppImage,
  then drivers, firmware and language package managers.
- Arch / CachyOS first, with Debian (apt) and Fedora (dnf) planned from the start.
- Open source (GPLv3).

Not goals (for now): hosting packages ourselves, paid apps, user accounts or
online ratings. linux-store consumes existing sources; it does not replace them.

## How it is built

A Rust workspace, in three layers:

```
gui  (Slint)  ──►  core  ──►  backends: pacman · aur · flatpak · apt · dnf · …
                     │
                     └──►  helper (polkit): the only part that runs as root
```

- **`core`**: a common `Backend` interface (search, details, install, remove,
  update, list installed), a shared package model, an operation queue with progress
  and cancellation, and a local SQLite index for instant, offline search.
- **Backends**: one adapter per source, so a new source never requires changing the
  GUI. Arch (pacman, AUR) and Flatpak come first; apt and dnf are reserved slots.
- **Privileged helper**: installs go through polkit. The GUI never runs as root.
- **`gui`**: the [Slint](https://slint.dev) interface, talking only to `core`.

Only the sources available on the current distro are shown.

The full reasoning is in the [design document](docs/2026-11-18-linux-store-design.md).

## Roadmap

1. `core`, the SQLite index and the pacman backend, with a test CLI
2. AUR and Flatpak backends (read-only: done)
3. The Slint GUI: search, app pages, install/remove, updates
4. Privileged helper (polkit): written and tested with fixtures, awaiting the manual check
5. apt and dnf backends
6. Snap, AppImage, firmware (fwupd), drivers, language package managers

## Repository layout

| Path | What it is |
|---|---|
| `core/` | `store-core`: package model, `Backend` trait, SQLite search index, `Store` |
| `backends/pacman/` | Read-only pacman backend (libalpm) |
| `backends/aur/` | Read-only AUR backend (official metadata dump, streamed) |
| `backends/flatpak/` | Read-only Flatpak backend (AppStream catalog) |
| `protocol/` | `store-protocol`: the one-line JSON requests and events, and all input validation |
| `helper/` | `store-helper`: the privileged helper (libalpm transactions); the only code that runs as root |
| `installers/pkexec/` | `Installer` implementation that runs the helper through `pkexec` |
| `packaging/` | The polkit policy and the dev install and check scripts |
| `cli/` | `store` command line: `refresh`, `search`, `installed`, `install`, `remove`, `upgrade` |
| `gui/` | The Slint application |
| `docs/` | Design documents |
| `legacy/` | The original bash/yad `stoa-store`, kept as reference only |

## Build and run

You need a recent stable Rust toolchain ([rustup](https://rustup.rs)).

```sh
cargo build
cargo run
```

**Hybrid NVIDIA + AMD laptops:** a plain `cargo run` hard-froze one development
machine (the default OpenGL renderer most likely went through the NVIDIA driver).
If you have a hybrid GPU, prefer one of these:

```sh
# CPU rendering
SLINT_BACKEND=winit-software cargo run

# Mesa (AMD/Intel) EGL only
__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json cargo run
```

Details are in [`AGENTS.md`](AGENTS.md).

### Try the core from the command line

```sh
cargo run --release -p store-cli -- refresh      # index pacman, Flatpak and the AUR
cargo run --release -p store-cli -- search firefox
cargo run --release -p store-cli -- installed
```

`refresh`, `search` and `installed` are read-only: they never write to pacman's or Flatpak's
data. The AUR dump is downloaded on every refresh (about 15 MB). On an Arch system with about 145,000
packages in total (pacman, Flatpak and the AUR), refresh takes about 5 seconds, mostly the
download, and a typical search about 10 ms.

### Changing the system (needs the helper)

`store install <names>`, `store remove <names>` and `store upgrade` change the system through a
small root helper started with `pkexec`. They need the helper installed first, which puts two
root-owned files on your machine (a binary and a polkit policy):

```sh
cargo build --release -p store-helper -p store-cli
sudo sh packaging/dev-install.sh        # installs them; undo with packaging/dev-uninstall.sh
cargo run --release -p store-cli -- install tree
```

The password is asked once and remembered for about five minutes by the process that asked.
`packaging/manual-check.sh` walks through installing and removing two tiny packages to check
the real prompt. The helper accepts only package names (never paths or options), refuses to
remove `pacman`, `base` and what `base` needs, and refreshes the package databases only as
part of an upgrade, never alone.

## Continuous integration

Every push to `main` and every pull request runs (see `.github/workflows/`):

- `cargo fmt --check`
- `cargo clippy -D warnings` and the full test suite on Arch Linux
- a compile check of the Slint GUI (it is never run in CI)
- a weekly and per-dependency-change `cargo audit`; Dependabot proposes weekly updates

Run the same checks locally before pushing:

```sh
cargo fmt --all --check
cargo clippy --workspace --exclude linux-store --all-targets --features store-core/testing -- -D warnings
cargo test --workspace --exclude linux-store --features store-core/testing
```

## Contributing

The project is young and the architecture is still taking shape, so the best way
to help right now is to read the [design document](docs/2026-11-18-linux-store-design.md)
and open an issue with feedback or ideas.

## Resources

- [Slint documentation](https://docs.slint.dev/latest/docs/slint/)

## License

GPLv3, see [`LICENSE`](LICENSE). Slint is used under its GPLv3 option, see
[`LICENSE-SLINT`](LICENSE-SLINT).
