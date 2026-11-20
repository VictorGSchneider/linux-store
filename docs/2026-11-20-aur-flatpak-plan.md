# AUR and Flatpak Backends Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two more read-only sources, the AUR (from the official metadata dump) and Flatpak (from the AppStream catalog), searchable next to pacman through the same index and CLI.

**Architecture:** Each source is a crate implementing the existing `Backend` trait. The AUR backend streams `packages-meta-ext-v1.json.gz` straight from the network into a JSON parser. The Flatpak backend streams the AppStream XML Flatpak already keeps on disk. `Package` gains a display `title` (needed for Flatpak, whose ids look like `org.mozilla.firefox`), which bumps the index schema to 2.

**Tech Stack:** `ureq` 3.5.2, `serde` 1.0.229 (derive), `serde_json` 1.0.154, `flate2` 1.1.11, `quick-xml` **0.41.0** (pinned: 0.42 changed the reader API), all MIT/Apache-2.0; dev: existing `tempfile`.

**Spec:** `docs/2026-11-18-linux-store-design.md` (stage 2 of "Etapas"; backends AUR and Flatpak). Builds on `docs/2026-11-18-core-index-pacman-plan.md`.

**Scope note:** Read paths only (refresh, search, list), like stage 1. Installing/removing, the polkit helper and the GUI are later plans. Also out of scope: AUR popularity/votes in ranking, conditional HTTP caching of the dump, Flatpak update detection (needs commit comparison), and merging an AUR package with the same package shown as a pacman "foreign" install.

## Global Constraints

- Branch protection on `main`: work on a branch and open a PR; CI (rustfmt, clippy `-D warnings`, tests on Arch, GUI check) must be green. Run before each commit: `cargo fmt --all`, then `cargo clippy --workspace --exclude linux-store --all-targets --features store-core/testing -- -D warnings`, then `cargo test --workspace --exclude linux-store --features store-core/testing`.
- Directory names: `backends/aur/` (package `store-backend-aur`), `backends/flatpak/` (package `store-backend-flatpak`). Source ids: `aur`, `flatpak`.
- Never parse CLI text output (no `flatpak list`, no `yay`). Never run as root; this plan performs no privileged operation and writes nothing outside the index and the temp dirs of tests.
- Streaming only: neither the AUR dump (about 125,000 entries) nor the AppStream file (48 MiB) may be loaded whole; peak extra memory stays in the single-digit MiB (spike: 2 and 3 MiB).
- Network code has timeouts (connect 10 s, whole request 120 s) and a generic `User-Agent: linux-store/<crate version>`.
- Tests never touch the real network or the real Flatpak directories; they use local fixtures and a loopback test server. The one test that does is `#[ignore]`.
- Never run the `gui` crate (see `AGENTS.md`).
- Each task ends with the three commands above green and one commit; messages end with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.

## Review Focus

1. An AUR download that is truncated, garbled, empty (`[]`) or served with a non-200 status must be an error that keeps the previous AUR data; it must never replace the catalog with nothing.
2. A Flatpak remote whose catalog file is missing or unparsable must be an error, not an empty success; a machine with no Flatpak remotes at all is "unavailable", not a failure.
3. The same app id appearing for several branches (461 of 3,388 apps in the real catalog) must not fail the refresh: one entry per id, preferring branch `stable`.
4. Searching `firefox` finds an app by its display title, and searching `org.mozilla` finds it by id.
5. An index file written by stage 1 (schema 1) opens and is rebuilt empty, instead of erroring; the next refresh refills it.

---

### Task 1: `Package.title`, index schema 2, CLI shows titles

**Files:**
- Modify: `core/src/package.rs`, `core/src/index.rs`, `core/src/testing.rs`, `core/src/store.rs` (test helpers), `backends/pacman/src/lib.rs`, `cli/src/lib.rs`, plus the tests that build `Package` literals (`core/src/index.rs`, `cli/tests/cli.rs`, `backends/pacman/tests/pacman.rs`)

**Interfaces:**
- Consumes: `Package`, `Index`, `Error` from stage 1.
- Produces:
  - `Package` gains `pub title: Option<String>` (human display name; `None` for pacman/AUR). `testing::pkg` sets it to `None`.
  - `Index` schema version 2: `packages.title TEXT` column; the FTS table covers `name`, `title`, `description`; `bm25` weights are name 10, title 10, description 1. A database with `user_version` 1 is dropped and recreated empty (it is a cache) and set to 2; version 0 is created; versions above 2 still return `Error::UnsupportedIndexVersion`. Opening an up-to-date index still takes no write lock.
  - CLI line format becomes `<source>/<repo-or-"-"> <name> <version>[ [installed]] — <title>: <description>` when `title` is present, otherwise unchanged (`— <description>`).

- [ ] **Step 1: Write the failing tests**
  - `core/src/index.rs`: `title_roundtrips` (a package with `title: Some("Firefox")` comes back equal from `get`); `search_finds_by_title` (package name `org.mozilla.firefox`, title `Firefox`: `search("firefox")` finds it, and so does `search("org.mozilla")`) *(Review Focus 4)*; `schema_1_index_is_rebuilt` (create a file with `PRAGMA user_version=1` and a stage 1 `packages` table holding one row, `Index::open` succeeds, `count() == 0`, user_version is 2) *(Review Focus 5)*; the existing `opening_a_newer_index_is_refused...` test now uses version 99 as before and must still pass.
  - `cli/tests/cli.rs`: `search_shows_title_before_description` (output contains `— Alpha App: first`).
- [ ] **Step 2: Run** `cargo test --workspace --exclude linux-store --features store-core/testing` — expected: compile errors / FAIL (`title` does not exist).
- [ ] **Step 3: Implement** the field and migration. The rebuild is: read `user_version`; if 1, `DROP TABLE packages; DROP TABLE packages_fts;` then run the schema script and set version 2.
- [ ] **Step 4: Run** the full test command — expected: all pass.
- [ ] **Step 5: Commit** `core: add package title and index schema 2`.

---

### Task 2: AUR backend

**Files:**
- Modify: `Cargo.toml` (add `"backends/aur"` to `members`)
- Create: `backends/aur/Cargo.toml`, `backends/aur/src/lib.rs`, `backends/aur/tests/aur.rs`

**Interfaces:**
- Consumes: `Backend`, `Package`, `SourceId`, `Error` from `store-core`.
- Produces:
  - `pub enum DumpSource { Url(String), File(PathBuf) }`
  - `pub const DEFAULT_DUMP_URL: &str = "https://aur.archlinux.org/packages-meta-ext-v1.json.gz"`
  - `pub struct AurBackend`; `AurBackend::new() -> AurBackend` (default URL); `AurBackend::with_source(DumpSource) -> AurBackend`. Implements `Backend` with `source()` = `SourceId::new("aur")`; `is_available()` is true for `Url`, and for `File` iff the file exists.
  - `packages()`: streams the (gzipped) JSON array through a `serde` sequence visitor, one entry at a time, into `Package { source: aur, name: Name, version: Version, installed_version: None, title: None, description: Description, repo: Some("aur"), url: URL, download_size: 0, installed_size: 0 }`. Unknown JSON fields are ignored. Errors: a transport or non-200 error, a gzip or JSON error (including truncation), and a dump that yields **zero** entries each return `Error::Backend { source_id: "aur", message }` with a message that names the cause. Duplicate names inside one dump keep the first.

- [ ] **Step 1: Write the failing tests** in `backends/aur/tests/aur.rs`, with a helper that gzips a JSON string to a temp file:
  - `parses_entries`: fixture with 2 entries (one with `Description: null`) → names, versions, `repo == Some("aur")`, description `None` for the null one, `url` kept.
  - `ignores_unknown_fields`: an entry with extra keys parses.
  - `truncated_gzip_is_an_error`: first half of the gzipped bytes → `Err(Error::Backend{..})`. *(Review Focus 1)*
  - `garbled_json_is_an_error`, `empty_array_is_an_error` (message mentions "no packages"). *(Review Focus 1)*
  - `duplicate_names_keep_the_first`.
  - `fetches_over_http`: a `std::net::TcpListener` on `127.0.0.1:0` in a thread serves the gzipped fixture with status 200 once; `with_source(Url(..))` returns the same packages.
  - `non_200_status_is_an_error`: the server answers `503`; `Err(Error::Backend{..})`. *(Review Focus 1)*
  - `missing_file_is_unavailable`: `File("/nonexistent")` → `is_available() == false`.
  - `real_aur_smoke` marked `#[ignore]`: `AurBackend::new().packages()` returns more than 50,000 entries.
- [ ] **Step 2: Run** `cargo test -p store-backend-aur` — expected: FAIL.
- [ ] **Step 3: Implement** `AurBackend`. For `Url`, build a `ureq` agent with the timeouts and `User-Agent`, and feed the response body reader into `flate2::read::GzDecoder` then `serde_json::Deserializer::from_reader`, calling `deserialize_seq` with a visitor that pushes `Package`s (it must not collect the raw entries first).
- [ ] **Step 4: Run** `cargo test -p store-backend-aur`, then `cargo test -p store-backend-aur -- --ignored` — expected: 9 passed (+1 ignored), then the smoke test passes on a networked machine.
- [ ] **Step 5: Commit** `backends/aur: index the AUR from the official metadata dump`.

---

### Task 3: Flatpak backend (AppStream catalog)

**Files:**
- Modify: `Cargo.toml` (add `"backends/flatpak"`)
- Create: `backends/flatpak/Cargo.toml`, `backends/flatpak/src/lib.rs`, `backends/flatpak/tests/flatpak.rs`

**Interfaces:**
- Consumes: as Task 2.
- Produces:
  - `pub struct FlatpakBackend`; `FlatpakBackend::from_system() -> FlatpakBackend` (installations `/var/lib/flatpak` and `$HOME/.local/share/flatpak`, arch from `std::env::consts::ARCH`); `FlatpakBackend::with_installations(dirs: &[PathBuf], arch: &str) -> FlatpakBackend`. `source()` = `flatpak`.
  - Catalog location: `<installation>/appstream/<remote>/<arch>/active/appstream.xml` (or `.xml.gz`); `<remote>` is every sub-directory name. `is_available()` is true iff at least one such catalog file exists.
  - `packages()` streams each catalog with `quick-xml` and keeps components of type `desktop-application` and `console-application` only (runtimes, addons, localization and others are skipped). Per component: `name` = `<id>`, `title` = `<name>` without an `xml:lang`, `description` = `<summary>` without an `xml:lang`, `version` = the version of the release with the highest `timestamp` attribute (falls back to the `<bundle>` branch when there are no releases), `url` = `<url type="homepage">`, `repo` = the remote name, `download_size` = `installed_size` = 0 (not in AppStream). The branch comes from `<bundle type="flatpak">app/<id>/<arch>/<branch></bundle>`.
  - One entry per id: when an id occurs several times (branches), keep the `stable` branch, else the first seen.
  - Installed detection: an app is installed when `<installation>/app/<id>/<arch>/<branch>/active` exists for the kept branch (a symlink to the deployed commit; this layout is the documented Flatpak one but **could not be verified on the dev machine, which has no Flatpak apps**, so the fixture encodes it and the `#[ignore]` test below checks it on a machine that has one). `installed_version` is `Some(<catalog version>)`: the real installed version is unknown until update detection exists (out of scope).
  - Errors: a remote directory whose catalog file is missing, unreadable, or not well-formed XML returns `Error::Backend { source_id: "flatpak", message }` naming the remote. A machine with no catalog at all makes `is_available()` false.

- [ ] **Step 1: Write the failing tests** with a fixture builder that writes a small `appstream.xml` (and a `.xml.gz` variant) and optional installed dirs under a temp installation:
  - `keeps_only_applications`: components of types `desktop-application`, `console-application`, `runtime`, `addon` → only the first two returned.
  - `maps_fields`: id `org.example.App`, name `Example` (plus a `xml:lang="pl"` name that must be ignored), summary, homepage URL, releases `1.0` (timestamp 100) and `1.1` (timestamp 200) → `name == "org.example.App"`, `title == Some("Example")`, `version == "1.1"`, `repo == Some("flathub")`.
  - `branches_collapse_to_one_entry_preferring_stable`: the same id with branches `beta` (first) and `stable` → one entry whose version is the stable one. *(Review Focus 3)*
  - `no_releases_falls_back_to_branch`.
  - `marks_installed_apps`: `app/org.example.App/x86_64/stable/active` present → `installed_version.is_some()` for that app only.
  - `reads_gzipped_catalogs` and `reads_user_and_system_installations` (same id in two installations still yields one entry).
  - `broken_xml_is_an_error_naming_the_remote`: truncated XML → `Err(Error::Backend{ message, .. })` with `message` containing the remote name. *(Review Focus 2)*
  - `no_catalog_means_unavailable`.
  - `real_system_smoke` `#[ignore]`: `from_system().packages()` returns more than 1,000 apps on a machine with the Flathub catalog.
- [ ] **Step 2: Run** `cargo test -p store-backend-flatpak` — expected: FAIL.
- [ ] **Step 3: Implement** the backend with `quick-xml` 0.41.0 events (`Reader::from_reader` over a `BufReader`, gzip via `flate2` when the file ends in `.gz`), tracking element depth so only direct children of `<component>` are read and language-tagged elements are skipped.
- [ ] **Step 4: Run** `cargo test -p store-backend-flatpak` and, on the dev machine, `cargo test -p store-backend-flatpak -- --ignored` — expected: all pass.
- [ ] **Step 5: Commit** `backends/flatpak: index Flathub apps from the AppStream catalog`.

---

### Task 4: Wire the sources into the CLI and verify end to end

**Files:**
- Modify: `cli/Cargo.toml`, `cli/src/main.rs`, `README.md`, `AGENTS.md`

**Interfaces:**
- Consumes: `AurBackend::new`, `FlatpakBackend::from_system`, `PacmanBackend::from_system`.
- Produces: the `store` binary builds its `Store` with all three backends, in the order pacman, flatpak, aur. `README.md` roadmap and layout list the new crates; `AGENTS.md` layout lists them.

- [ ] **Step 1: Write the failing test** `cli/tests/cli.rs::refresh_reports_every_source`: a `run_cli` with two fake backends (`one`, `two`) prints a line for each (`one: …`, `two: …`). (Covers multi-source reporting; the real wiring is in `main.rs`.)
- [ ] **Step 2: Run** the test command — expected: it may already pass, which means the CLI was already source-agnostic; keep it as a regression test and note that in the commit.
- [ ] **Step 3: Implement** the three-backend wiring in `main.rs` and update the docs.
- [ ] **Step 4: Run** the full test command — expected: all pass.
- [ ] **Step 5: End-to-end on this machine** (release build, scratch index only):
  `cargo build --release -p store-cli`, then with `DB=/tmp/store-e2e2.db`:
  `store --db $DB refresh` — expected: a line for `pacman`, `flatpak` and `aur` (about 16,600, about 2,900 and about 125,000 packages; network failure shows `aur: failed: …`, not a crash);
  `store --db $DB search firefox` — expected: the `pacman` `firefox` first, the Flatpak `org.mozilla.firefox` listed with title `Firefox`;
  `store --db $DB search paru` — expected: an `aur/aur` result;
  time `refresh` twice and `search firefox`, `search a`; print the index size. Report all numbers; if a search exceeds 50 ms or the index exceeds 100 MiB, stop and report instead of committing.
- [ ] **Step 6: Commit** `cli: search pacman, Flatpak and the AUR together`.

---

## Self-Review notes

- **Spec coverage:** stage 2 (AUR and Flatpak backends) of the design. Deferred and named in the scope note: install/remove, helper, GUI, update detection, AUR popularity ranking, HTTP caching, AUR/pacman "installed" merge.
- **Type consistency:** `Package.title: Option<String>` is introduced in Task 1 and used unchanged in Tasks 2 and 3 (AUR sets `None`, Flatpak sets `Some`); source ids `aur` and `flatpak` match the CLI output examples.
- **Decisions I made (not in the spec):** Flatpak `name` is the app id with the display name in `title`; only desktop and console applications are listed; one entry per id preferring `stable`; the AUR is read from the metadata dump, not the per-package RPC; the dump is streamed into the parser with no temp file; `quick-xml` is pinned to 0.41.0; an AUR dump with zero entries is an error; schema 1 is rebuilt, not migrated; Flatpak installed state is directory presence with the catalog version, until update detection exists.
