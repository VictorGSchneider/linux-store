# linux-store

A universal, open-source app store for Linux. One fast, simple GUI to search and
install software from every source: distro repos (pacman, apt, dnf), the AUR,
Flatpak, Snap, AppImage, and later drivers, firmware and programming-language
toolchains.

Written in Rust, with a [Slint](https://slint.dev) interface.

**Status:** early development. See the [design](docs/2026-11-18-linux-store-design.md).

## Layout

- `gui/` — the Slint application
- `docs/` — design documents
- `legacy/` — the original bash/yad `stoa-store`, kept as reference

## Build and run

```sh
cargo run
```

## License

GPLv3 (see `LICENSE`). Slint is used under its GPLv3 option (see `LICENSE-SLINT`).
