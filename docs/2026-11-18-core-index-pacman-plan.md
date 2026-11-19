# Core, Index and Pacman Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `store-core` library (package model, `Backend` trait, SQLite search index, `Store`), a read-only pacman backend, and a `store` CLI that proves them end to end on a real Arch/CachyOS system.

**Architecture:** `core` defines a read-only `Backend` trait and owns a local SQLite (FTS5) index. A `Store` refreshes the index from every available backend and answers searches from the index only, so search never touches a backend. The pacman backend reads the system's pacman databases through libalpm (no CLI parsing). The CLI is a thin consumer used for verification.

**Tech Stack:** Rust 1.100 / edition 2024, `rusqlite` 0.43.0 (feature `bundled`), `thiserror` 2.0.21, `alpm` 5.0.2, `alpm-utils` 5.0.0, `pacmanconf` 3.1.0, `clap` 4.6.8; dev: `tempfile` 3.27.0, `tar` 0.4.46, `flate2` 1.1.11.

**Spec:** `docs/2026-11-18-linux-store-design.md` (sections "Arquitetura", "Etapas" 1 and the pacman part of the backend list).

**Scope note:** The design says the first plan covers stages 1 and 4. This plan covers stage 1 only, **read paths** (refresh, search, list installed). The privileged helper, `install`/`remove`/`update`, and the operation queue with progress and cancellation are the next plan; they need polkit design of their own. Backend mutation methods are therefore *not* on the trait yet.

## Global Constraints

- Rust workspace; GPLv3 project. All new crates are GPL-3.0 compatible (listed dependencies are MIT/Apache-2.0 or GPL-3.0).
- Directory `core/` holds package `store-core` (lib `store_core`). A package named `core` would collide with Rust's built-in `core`.
- Adding a backend must never require changing GUI code; `core` knows no concrete backend.
- No parsing of CLI text output. pacman data comes from libalpm only.
- Never run as root; this plan performs no privileged operation and never writes to pacman's databases.
- Fast and light: indexing about 16,000 packages must stay well under 1 s (measured 143 ms in a spike) and a search under 10 ms (measured 0.34 ms).
- Run the GUI only with `SLINT_BACKEND=winit-software` or Mesa EGL (see `AGENTS.md`). This plan does not run the GUI.
- Every task ends with `cargo test --workspace` green and one commit. Commit messages end with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.

## Review Focus

1. Search text containing FTS5 syntax (`"`, `*`, `-`, `(`, `:`, `AND`, `NEAR(`) or `c++` must never return an error: results or an empty list.
2. Empty or whitespace-only query returns an empty list, never the whole catalog.
3. Searching `firefox` returns the package named exactly `firefox` first, ahead of `firefox-i18n-*`.
4. A backend that is unavailable or fails during refresh must not erase that source's previously indexed data, and must not stop other backends from refreshing.
5. Case and accents: searching `ACAO` finds a description containing `ação`.

---

### Task 1: Workspace member `core`: model, `Backend` trait, errors, fake backend

**Files:**
- Modify: `Cargo.toml` (add `"core"` to `members`)
- Create: `core/Cargo.toml`, `core/src/lib.rs`, `core/src/package.rs`, `core/src/backend.rs`, `core/src/error.rs`, `core/src/testing.rs`

**Interfaces:**
- Consumes: nothing.
- Produces (all re-exported from `store_core`):
  - `pub struct SourceId(String)` — derives `Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord`; `SourceId::new(impl Into<String>) -> SourceId`; `as_str(&self) -> &str`; `impl Display`.
  - `pub struct Package { pub source: SourceId, pub name: String, pub version: String, pub installed_version: Option<String>, pub description: Option<String>, pub repo: Option<String>, pub url: Option<String>, pub download_size: u64, pub installed_size: u64 }` — derives `Clone, Debug, PartialEq, Eq`; `fn is_installed(&self) -> bool` is true iff `installed_version.is_some()`. `version` is the newest version the source knows (for installed-only packages, the installed version).
  - `pub trait Backend: Send + Sync { fn source(&self) -> SourceId; fn is_available(&self) -> bool; fn packages(&self) -> Result<Vec<Package>, Error>; }` — `packages()` returns everything the source knows: catalog entries with `installed_version` filled in where installed, plus installed packages absent from the catalog (`repo: None`).
  - `pub enum Error { Backend { source_id: SourceId, message: String }, Index(rusqlite::Error), SourceMismatch { expected: SourceId, found: SourceId }, Io(std::io::Error) }` with `thiserror`; `From<rusqlite::Error>` and `From<std::io::Error>`. (The field is `source_id`, not `source`, because thiserror treats `source` as the error cause.) `pub type Result<T> = std::result::Result<T, Error>`.
  - Behind cargo feature `testing`: `store_core::testing::FakeBackend` with `FakeBackend::new(source: &str, packages: Vec<Package>) -> FakeBackend`, `.unavailable(self) -> FakeBackend`, `.failing(self, message: &str) -> FakeBackend`; and `testing::pkg(source: &str, name: &str, description: &str) -> Package` (version `"1.0"`, not installed, other fields empty/zero). `FakeBackend::is_available()` is false after `.unavailable()`; `packages()` returns `Error::Backend` after `.failing()`.

- [ ] **Step 1: Write the failing tests** in `core/src/package.rs` and `core/src/testing.rs` (`#[cfg(test)]`):
  - `package_is_installed_follows_installed_version`: `pkg(..)` is not installed; with `installed_version = Some("1.0".into())` it is.
  - `fake_backend_returns_its_packages`: `FakeBackend::new("fake", vec![pkg("fake","a","x")]).packages()` equals that vec; `source()` is `SourceId::new("fake")`; `is_available()` is true.
  - `fake_failing_backend_returns_backend_error`: `.failing("boom").packages()` matches `Err(Error::Backend { message, .. })` with `message == "boom"`.
  - `fake_unavailable_backend_reports_unavailable`.
- [ ] **Step 2: Run** `cargo test -p store-core --features testing` — expected: compile errors / FAIL (nothing exists yet).
- [ ] **Step 3: Implement** the interfaces above. `core/Cargo.toml` declares `[features] testing = []`; `rusqlite` with `bundled`; `thiserror`; dev-dependency `tempfile`.
- [ ] **Step 4: Run** `cargo test -p store-core --features testing` — expected: 4 passed.
- [ ] **Step 5: Commit** `core: add package model, Backend trait and error type`.

---

### Task 2: SQLite index: schema, `replace_source`, `get`, `installed`, `count`

**Files:**
- Create: `core/src/index.rs`
- Modify: `core/src/lib.rs` (`mod index; pub use index::Index;`)

**Interfaces:**
- Consumes: `Package`, `SourceId`, `Error`, `Result` from Task 1.
- Produces, in `store_core::Index`:
  - `Index::open(path: &Path) -> Result<Index>` (creates the file and schema if missing and sets `journal_mode=WAL`; the caller is responsible for the parent directory existing)
  - `Index::open_in_memory() -> Result<Index>`
  - `fn replace_source(&mut self, source: &SourceId, packages: &[Package]) -> Result<()>` — one transaction: delete all rows of `source`, insert `packages`. If any package's `source` differs from `source`, return `Error::SourceMismatch { expected, found }` and leave the previous data untouched.
  - `fn get(&self, source: &SourceId, name: &str) -> Result<Option<Package>>`
  - `fn installed(&self) -> Result<Vec<Package>>` — only installed packages, across sources, sorted by `name` then `source`.
  - `fn count(&self) -> Result<usize>`
  - Schema: table `packages` with primary key `(source, name)` holding every `Package` field; plus an FTS5 virtual table over `name` and `description` (tokenizer `unicode61`, which folds case and diacritics), kept in sync inside `replace_source`'s transaction. `PRAGMA user_version` is set to `1`.

- [ ] **Step 1: Write the failing tests** (in-memory unless stated):
  - `roundtrip_preserves_every_field`: insert one `Package` with all fields set (including `None` description/url/repo in a second package); `get` returns equal packages.
  - `replace_source_replaces_only_that_source`: index holds `a:x` and `b:y`; `replace_source("a", [a:z])` → `get(a,x)` is `None`, `get(a,z)` is `Some`, `get(b,y)` still `Some`; `count() == 2`.
  - `replace_source_is_atomic_on_mismatch`: index holds `a:x`; `replace_source("a", [a:ok, b:bad])` returns `Err(Error::SourceMismatch{..})`; `get(a,x)` still `Some` and `get(a,ok)` is `None`.
  - `installed_lists_only_installed_sorted`: three packages (two installed, names `zsh`, `bash`) → `["bash","zsh"]`.
  - `index_persists_on_disk` (uses `tempfile`): write via `open(path)`, drop, reopen, `count()` unchanged.
- [ ] **Step 2: Run** `cargo test -p store-core --features testing index` — expected: FAIL (`Index` undefined).
- [ ] **Step 3: Implement** `Index` as specified. Store `download_size`/`installed_size` as `INTEGER`; use prepared statements inside the transaction.
- [ ] **Step 4: Run** the same command — expected: 5 passed.
- [ ] **Step 5: Commit** `core: add SQLite index with atomic per-source replace`.

---

### Task 3: Index search with ranking and safe query handling

**Files:**
- Modify: `core/src/index.rs`

**Interfaces:**
- Consumes: `Index` from Task 2.
- Produces: `fn search(&self, query: &str, limit: usize) -> Result<Vec<Package>>` on `Index`.
  - Query handling: split `query` into terms on any non-alphanumeric character; a query with no terms returns `Ok(vec![])`. Each term is passed to FTS5 as a quoted string literal (never as raw syntax); the **last** term is a prefix match; terms are ANDed. Matches in `name` or `description`.
  - Ordering: (1) case-insensitive exact name match, (2) name starts with the whole query, (3) FTS `bm25` with `name` weighted 10× over `description`, then (4) shorter name, then name ascending.
  - At most `limit` rows.

- [ ] **Step 1: Write the failing tests** (in-memory index, helper `pkg` from `testing`):
  - `search_finds_by_name_prefix`: `firefox`, `firefox-i18n-fr`, `git`; `search("fire", 10)` → both firefox names, not `git`.
  - `exact_name_ranks_first`: inserted order `firefox-i18n-fr`, `firefox-developer-edition`, `firefox`; `search("firefox", 10)[0].name == "firefox"`. *(Review Focus 3)*
  - `search_matches_description`: `git` with description "the fast distributed version control system"; `search("distributed")` finds it.
  - `search_requires_all_terms`: `search("fast distributed")` finds `git`; `search("fast banana")` is empty.
  - `search_is_case_and_accent_insensitive`: package `editor-pt` with description `Editor de ação`; `search("ACAO")` finds it. *(Review Focus 5)*
  - `search_survives_fts_syntax`: for each of `"\""`, `"firefox\""`, `"a AND"`, `"("`, `"foo:bar"`, `"-x"`, `"*"`, `"NEAR("`, `"c++"`: `search(q, 10)` is `Ok(_)`. *(Review Focus 1)*
  - `empty_query_returns_nothing`: `""` and `"   "` → empty vec, with a non-empty index. *(Review Focus 2)*
  - `search_respects_limit`: 20 matching packages, `limit = 5` → 5 results.
- [ ] **Step 2: Run** `cargo test -p store-core --features testing search` — expected: FAIL (`search` undefined).
- [ ] **Step 3: Implement** `Index::search` per the semantics above. The exact-name and prefix tiers can be `ORDER BY` expressions over `lower(name)`.
- [ ] **Step 4: Run** the same command — expected: 8 passed.
- [ ] **Step 5: Commit** `core: add ranked full-text search to the index`.

---

### Task 4: `Store`: refresh from backends and search

**Files:**
- Create: `core/src/store.rs`
- Modify: `core/src/lib.rs` (`mod store; pub use store::{Store, RefreshReport, SourceReport, Outcome};`)

**Interfaces:**
- Consumes: `Backend`, `Index`, `FakeBackend` (dev only).
- Produces:
  - `pub struct Store` — `Store::new(index: Index, backends: Vec<Box<dyn Backend>>) -> Store`
  - `fn refresh(&mut self) -> RefreshReport` — for each backend, in order: if `!is_available()` → `Outcome::Unavailable` and its index data is left untouched; else call `packages()`; on `Ok` → `index.replace_source` and `Outcome::Updated { packages: n }`; on `Err` (from the backend **or** from `replace_source`) → `Outcome::Failed(message)` with old data untouched. One backend's outcome never affects another's.
  - `fn search(&self, query: &str, limit: usize) -> Result<Vec<Package>>` and `fn installed(&self) -> Result<Vec<Package>>` delegating to the index.
  - `pub struct RefreshReport { pub sources: Vec<SourceReport> }`, `pub struct SourceReport { pub source: SourceId, pub outcome: Outcome }`, `pub enum Outcome { Updated { packages: usize }, Unavailable, Failed(String) }` (all `Debug, Clone, PartialEq, Eq`).

- [ ] **Step 1: Write the failing tests** (`#[cfg(test)]`, using `FakeBackend`):
  - `refresh_indexes_available_backends`: one fake with 2 packages → report `[Updated{packages:2}]`; `search("alpha")` finds the package.
  - `failing_backend_does_not_block_others`: fakes `bad` (failing) and `good` (1 package) → report is `[Failed(_), Updated{packages:1}]` in that order; `good`'s package is searchable.
  - `unavailable_backend_keeps_old_data` *(Review Focus 4)*: shared on-disk index in a `tempdir`; store A refreshes with an available fake of source `s` holding package `keep`; store B (same index file, fake of source `s` `.unavailable()`) refreshes → `Unavailable`; `search("keep")` on B still finds it.
  - `failed_refresh_keeps_old_data` *(Review Focus 4)*: same setup with B `.failing("net down")` → `Failed("net down")`-style outcome containing `"net down"`, old data intact.
  - `installed_comes_from_the_index`: a fake with one installed and one not-installed package → `installed()` returns only the installed one.
- [ ] **Step 2: Run** `cargo test -p store-core --features testing store` — expected: FAIL.
- [ ] **Step 3: Implement** `Store` as specified.
- [ ] **Step 4: Run** `cargo test --workspace --features store-core/testing` — expected: all pass.
- [ ] **Step 5: Commit** `core: add Store that refreshes the index from backends`.

---

### Task 5: pacman backend (read-only, via libalpm)

**Files:**
- Modify: `Cargo.toml` (add `"backends/pacman"` to `members`)
- Create: `backends/pacman/Cargo.toml` (package `store-backend-pacman`), `backends/pacman/src/lib.rs`, `backends/pacman/tests/fixture.rs`

**Interfaces:**
- Consumes: `Backend`, `Package`, `SourceId`, `Error` from `store-core`.
- Produces:
  - `pub struct PacmanBackend` implementing `Backend`; `source()` is `SourceId::new("pacman")`.
  - `PacmanBackend::from_system() -> PacmanBackend` — reads `/etc/pacman.conf` with `pacmanconf::Config::new()` and opens a libalpm handle with `alpm_utils::alpm_with_conf`.
  - `PacmanBackend::with_paths(root: &Path, db_path: &Path, repos: &[&str]) -> PacmanBackend` — registers the named sync repos with `SigLevel::NONE`; used by tests.
  - `is_available()`: true iff a handle can be opened (config readable, db path exists). Never panics.
  - `packages()`: opens a **fresh** `Alpm` handle per call (the handle is not `Send`/`Sync`), reads every sync db package (first repo wins on duplicate names), then local db: `installed_version` set when the local db has the name; local packages not in any sync db are added with `repo: None`. Field mapping: `version` = sync version (installed version for installed-only), `description` = `desc()`, `repo` = db name, `url` = `url()`, `download_size` = `size()` (compressed size; do **not** use `download_size()`, which is 0 for cached or fixture packages), `installed_size` = `isize()`. libalpm errors map to `Error::Backend { source_id: "pacman", .. }`.
- Fixture facts (verified in a spike): libalpm derives `name`/`version` of a sync entry from its **directory name** `name-version` inside the `.db` tarball, so folder names must match `%NAME%`/`%VERSION%`; the local db needs `local/ALPM_DB_VERSION` containing `9`; the local `desc` uses `%SIZE%` for installed size. `tests/fixture.rs` builds a temp root with `tar` + `flate2`: sync repo `core` containing `hello-2.0-1` (desc "Hello tool", url, `%CSIZE%` 1234, `%ISIZE%` 5678) and `world-2.0-1` (no desc), and a local db with `hello-1.0-1` and `foreign-3-1`.

- [ ] **Step 1: Write the failing tests** in `backends/pacman/tests/pacman.rs` using the fixture builder:
  - `lists_sync_packages_with_metadata`: `hello` has `version == "2.0-1"`, `description == Some("Hello tool")`, `repo == Some("core")`, `url == Some(..)`, `download_size == 1234`, `installed_size == 5678`.
  - `marks_installed_version`: `hello.installed_version == Some("1.0-1")`; `world.installed_version == None`.
  - `includes_foreign_installed_packages`: `foreign` present with `repo == None`, `installed_version == Some("3-1")`.
  - `source_is_pacman`.
  - `missing_db_is_unavailable_not_a_panic`: `with_paths` on a non-existent dir → `is_available() == false`.
  - `real_system_smoke` marked `#[ignore]` (run manually): `from_system().packages()` is `Ok` with more than 1000 packages.
- [ ] **Step 2: Run** `cargo test -p store-backend-pacman` — expected: FAIL.
- [ ] **Step 3: Implement** `PacmanBackend` as specified.
- [ ] **Step 4: Run** `cargo test -p store-backend-pacman` — expected: 5 passed, 1 ignored; then `cargo test -p store-backend-pacman -- --ignored` — expected: passed on this machine.
- [ ] **Step 5: Commit** `backends/pacman: read packages from pacman databases via libalpm`.

---

### Task 6: `store` CLI and end-to-end check on the real system

**Files:**
- Modify: `Cargo.toml` (add `"cli"` to `members`), `README.md` (layout table: add `core/`, `backends/`, `cli/`), `AGENTS.md` (Layout section)
- Create: `cli/Cargo.toml` (package `store-cli`, binary `store`), `cli/src/main.rs`, `cli/tests/cli.rs`

**Interfaces:**
- Consumes: `Store`, `Index`, `Outcome` from `store-core`; `PacmanBackend` from Task 5.
- Produces: binary `store`:
  - `store [--db <path>] refresh` — prints one line per source: `pacman: 15768 packages` / `pacman: unavailable` / `pacman: failed: <message>`; exit code 0 unless **every** source failed.
  - `store [--db <path>] search <query> [--limit N]` (default 20) — one line per result: `<source>/<repo-or-"-"> <name> <version> [installed] — <description>`.
  - `store [--db <path>] installed` — same line format.
  - Default `--db`: `$XDG_CACHE_HOME/linux-store/index.db`, else `$HOME/.cache/linux-store/index.db`; the parent directory is created. A path that cannot be opened prints a one-line error to stderr and exits 1 (no panic).
  - Library function `fn run(args: &[String], store: Store, out: &mut impl Write) -> i32` so tests can inject a store with `FakeBackend`; `main` builds the real store with `PacmanBackend::from_system()`.

- [ ] **Step 1: Write the failing tests** in `cli/tests/cli.rs` (dev-dependency `store-core` with feature `testing`):
  - `search_prints_matching_lines`: store with fake packages; after `refresh`, `search alpha` output contains `fake/- alpha 1.0`.
  - `search_with_no_match_prints_nothing_and_exits_zero`.
  - `refresh_reports_unavailable_source`: unavailable fake → output contains `fake: unavailable`.
  - `unwritable_db_path_is_a_clean_error`: `--db /proc/nope/index.db` → exit code 1, nothing panics.
- [ ] **Step 2: Run** `cargo test -p store-cli` — expected: FAIL.
- [ ] **Step 3: Implement** the CLI with `clap` (derive). Update `README.md` and `AGENTS.md` layout sections to list the new crates.
- [ ] **Step 4: Run** `cargo test --workspace --features store-core/testing` — expected: all pass.
- [ ] **Step 5: End-to-end on this machine** (release build; read-only, no GPU involved):
  `cargo build --release -p store-cli` then
  `time target/release/store --db /tmp/store-e2e.db refresh` — expected: `pacman: <~15,000+> packages`, wall time well under 1 s;
  `time target/release/store --db /tmp/store-e2e.db search firefox` — expected: first line is `pacman/extra firefox …`, wall time under 50 ms;
  `target/release/store --db /tmp/store-e2e.db installed | wc -l` — expected: close to `pacman -Q | wc -l`.
  Report the three measured numbers; if refresh exceeds 1 s or search exceeds 50 ms, stop and report instead of committing.
- [ ] **Step 6: Commit** `cli: add store CLI to refresh, search and list installed packages`.

---

## Self-Review notes

- **Spec coverage:** `core` trait/model, SQLite index, pacman via libalpm, Arch-first, apt/dnf left open by the trait (Task 1). Deferred on purpose and named in the scope note: install/remove/update, operation queue, polkit helper, AUR, Flatpak, GUI.
- **Type consistency:** `source_id` (not `source`) in `Error` variants everywhere; `Package` fields identical across tasks; `search` signature `(&self, &str, usize)` in Tasks 3, 4, 6.
- **Decisions I made (not in the spec):** the `store-core` package name; the read-only trait with a single `packages()` method; `installed_version: Option<String>` instead of a bool, so update detection fits later without a schema change; `size()` for download size; per-call libalpm handles.
