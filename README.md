# linux-store

**A universal, open-source app store for Linux.**

One fast, simple graphical app to find and install software from *every* source
on your system: your distro's own repositories, the AUR, Flatpak, Snap, AppImage,
and later drivers, firmware and programming-language toolchains.

Think Microsoft Store, App Store or Play Store, but open, light, and not tied to a
single package format.

> **Status: early development.** The workspace and a placeholder window exist; the
> store itself is not usable yet. See [Roadmap](#roadmap).

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
2. AUR and Flatpak backends
3. The Slint GUI: search, app pages, install/remove, updates
4. Privileged helper (polkit)
5. apt and dnf backends
6. Snap, AppImage, firmware (fwupd), drivers, language package managers

## Repository layout

| Path | What it is |
|---|---|
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

## Contributing

The project is young and the architecture is still taking shape, so the best way
to help right now is to read the [design document](docs/2026-11-18-linux-store-design.md)
and open an issue with feedback or ideas.

## Resources

- [Slint documentation](https://docs.slint.dev/latest/docs/slint/)

## License

GPLv3, see [`LICENSE`](LICENSE). Slint is used under its GPLv3 option, see
[`LICENSE-SLINT`](LICENSE-SLINT).
