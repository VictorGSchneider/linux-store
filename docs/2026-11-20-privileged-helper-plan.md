# Privileged Helper Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A small root helper (`store-helper`) that installs, removes and upgrades pacman packages through libalpm, a `pkexec` client behind an `Installer` trait, and `store install|remove|upgrade` commands, so the store can change the system with one password per about five minutes.

**Architecture:** `store-protocol` defines the one-line JSON request and event types and all input validation, shared by both sides. `store-helper` is a library plus a binary: operations run against an `Environment` (the real system, or a temporary root in tests), report progress as events, and are guarded by a `CancelGate`. `store-installer-pkexec` spawns `pkexec store-helper`, writes the request, and turns events and exit codes into typed results.

**Tech Stack:** Rust 1.100 / edition 2024; `alpm` 5.0.2, `alpm-utils` 5.0.0, `pacmanconf` 3.1.0, `serde` 1.0.229 (derive), `serde_json` 1.0.154, `thiserror` 2.0.21, `libc` 0.2.191, `signal-hook` 0.4.6; dev: `tempfile` 3.27.0, `quick-xml` 0.41.0. External tools for tests: `bsdtar`, `zstd`, `repo-add` (all ship with Arch's `base-devel`).

**Spec:** `docs/2026-11-20-privileged-helper-design.md`. Builds on `docs/2026-11-18-linux-store-design.md`.

## Global Constraints

- Work on a branch and open a PR; CI (rustfmt, clippy `-D warnings`, tests on Arch, GUI check) must be green. Before each commit run, in order: `cargo fmt --all`, then `cargo clippy --workspace --exclude linux-store --all-targets --features store-core/testing -- -D warnings`, then `cargo test --workspace --exclude linux-store --features store-core/testing`. **Never chain a commit after a failing clippy.**
- The GUI never runs as root; the helper is the only privileged code and refuses to run unless `geteuid() == 0`.
- The request carries only an operation and package **names**. The helper never accepts paths, URLs, repository names or options, never calls the `pacman` command or a shell, and never lowers the `SigLevel` from `pacman.conf`.
- `--root`, `--dbpath` and any config override exist **only** under `#[cfg(test)]` in the helper crate. No cargo feature, no environment variable and no flag may enable them in a build that ships.
- Names must match `^[a-z0-9@._+][a-z0-9@._+-]*$`, be at most 255 bytes, at most 200 per request, no duplicates. A request line is at most 64 KiB. Unknown JSON fields are rejected.
- libalpm transactions never use `TransFlag` values that skip signature or dependency checks. Scriptlets run in production; tests use `TransFlag::NO_SCRIPTLET` and an empty hooks directory.
- A transaction is atomic per request. The helper never deletes a `db.lck` it did not create and never leaves one it did.
- Tests are unprivileged: temporary root, `file://` fixture repository, no network. **Nothing in this plan runs `pkexec`, `sudo` or the real helper; the only privileged check is the manual script of Task 6, which the user runs themselves.** Never run the `gui` crate (see `AGENTS.md`).
- Dependencies must be GPL-3.0-compatible (MIT, Apache-2.0, GPL-3.0 are).
- Each task ends with the three commands above green and one commit; messages end with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.

## Review Focus

1. A hostile request line (`-x`, `../x`, `a b`, `a;b`, a newline or NUL inside a name, 201 names, duplicates, invalid UTF-8, an oversized line, a leading `cancel`) is rejected with exit code 2 **without ever opening libalpm**.
2. A transaction that fails (conflict, unknown name, broken dependency) leaves the local database exactly as it was and no `db.lck` behind.
3. Removing a protected package (`pacman`, `base` and what only it requires, `linux-store`, `linux-store-helper`) is refused even when asked directly; an unknown name returns `not_found` listing the names and installs nothing, even if other names in the request are valid.
4. Cancel: before the commit window it ends as `cancelled` and nothing changed; after the first package operation starts it answers `cancel_ignored` and the operation completes.
5. The client turns a garbage line, a helper that dies silently, exit 126, exit 127 and a missing program into typed errors, never a hang and never a panic.

---

### Task 1: `store-protocol`: requests, events, validation

**Files:**
- Modify: `Cargo.toml` (add `"protocol"` to `members`)
- Create: `protocol/Cargo.toml` (package `store-protocol`), `protocol/src/lib.rs`, `protocol/src/request.rs`, `protocol/src/event.rs`

**Interfaces:**
- Consumes: nothing.
- Produces (all re-exported from `store_protocol`):
  - `pub const MAX_PACKAGES: usize = 200; pub const MAX_NAME_BYTES: usize = 255; pub const MAX_REQUEST_BYTES: usize = 64 * 1024;`
  - `pub const EXIT_OK: i32 = 0; EXIT_FAILED: i32 = 1; EXIT_INVALID: i32 = 2; EXIT_CANCELLED: i32 = 3;` (126 and 127 are `pkexec`'s own: dismissed, not authorized).
  - `pub enum Request { Install { packages: Vec<String> }, Remove { packages: Vec<String> }, Upgrade, Cancel }`, JSON-tagged by `"op"` in `snake_case`, every variant `deny_unknown_fields`; `Request::parse(line: &[u8]) -> Result<Request, RequestError>` enforces the size limit, UTF-8, exactly one JSON object with no newline inside, the name rules, the counts, no duplicates, and non-empty lists for install and remove; `Request::to_line(&self) -> String` (JSON plus `\n`); `pub fn is_valid_name(name: &str) -> bool`; `RequestError` is a `thiserror` enum whose `Display` says what was wrong.
  - `pub enum Operation { Install, Remove, Upgrade }`, `pub enum Stage { Refresh, Resolve, Download, Verify, Install, Upgrade, Remove }`, `pub enum ChangeKind { Installed, Removed, Upgraded }`, `pub struct Change { pub package: String, pub kind: ChangeKind, pub version: Option<String> }`, `pub enum ErrorCode { InvalidRequest, NotFound, Conflict, Locked, Protected, DependencyBroken, Cancelled, TooLate, Failed }` (all `snake_case` in JSON).
  - `pub enum Event { Started { op: Operation }, Progress { stage: Stage, package: Option<String>, percent: Option<u8> }, Done { changes: Vec<Change> }, Error { code: ErrorCode, message: String }, CancelIgnored }`, tagged by `"event"` in `snake_case`; `Event::to_line(&self) -> String` and `Event::parse(line: &[u8]) -> Result<Event, EventError>` (same size limit).

- [ ] **Step 1: Write the failing tests** (in the two source files, `#[cfg(test)]`):
  - `parses_each_operation`: `{"op":"install","packages":["tree","sl"]}`, `{"op":"remove","packages":["tree"]}`, `{"op":"upgrade"}`, `{"op":"cancel"}` parse to the matching variants.
  - `rejects_hostile_names`: each of `"-x"`, `"--root=/x"`, `"../x"`, `"a/b"`, `"a b"`, `"a;b"`, `"a\\nb"`, `"a\\u0000b"`, `""`, `"Tree"`, `"é"`, and a 256-byte name inside an install request returns `Err`.
  - `rejects_too_many_or_duplicate_names`: 201 names, and `["a","a"]`, return `Err`; exactly 200 distinct names is accepted.
  - `rejects_empty_install_and_remove_lists`.
  - `rejects_unknown_fields_and_wrong_shapes`: `{"op":"upgrade","packages":[]}`, `{"op":"install","package":["a"]}`, `{"op":"format"}`, `[]`, `"install"`, `{"op":"install","packages":"tree"}`, two JSON objects on one line.
  - `rejects_oversized_invalid_utf8_and_blank_input`: a 64 KiB + 1 line, `b"\\xff\\xfe"`, `b""`, `b"   "`, and a line containing `\\n`.
  - `events_roundtrip_and_have_stable_json`: every `Event` variant survives `to_line` then `parse`; pin the shapes `{"event":"progress","stage":"download","package":"tree","percent":42}` and `{"event":"error","code":"not_found","message":"x"}`.
  - `exit_codes_are_distinct`.
- [ ] **Step 2: Run** `cargo test -p store-protocol` — expected: FAIL (crate does not exist).
- [ ] **Step 3: Implement** the interfaces above with `serde` derives; validation by a hand-written byte scan, no regex dependency.
- [ ] **Step 4: Run** `cargo test -p store-protocol` — expected: all pass.
- [ ] **Step 5: Commit** `protocol: add the helper request and event types`.

---

### Task 2: Helper environment, test fixture, `CancelGate` and `install`

**Files:**
- Modify: `Cargo.toml` (add `"helper"`)
- Create: `helper/Cargo.toml` (package `store-helper`, `[lib] name = "store_helper"`, `[[bin]] name = "store-helper"` with a placeholder `main`), `helper/src/lib.rs`, `helper/src/error.rs`, `helper/src/env.rs`, `helper/src/gate.rs`, `helper/src/events.rs`, `helper/src/install.rs`, `helper/src/fixture.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `Request`, `Event`, `ErrorCode`, `Change`, `Stage` from `store-protocol`.
- Produces:
  - `pub struct HelperError { pub code: ErrorCode, pub message: String }` (`Debug`, `Display`, `std::error::Error`); `pub type Emit<'a> = &'a mut dyn FnMut(Event);`
  - `pub struct Environment`; `Environment::system() -> Result<Environment, HelperError>` (reads `/etc/pacman.conf` with `pacmanconf::Config::new()` and opens handles with `alpm_utils::alpm_with_conf`, which applies `DownloadUser` and architectures); `pub(crate) fn open(&self) -> Result<Alpm, HelperError>`; `pub fn lock_path(&self) -> PathBuf` (`<dbpath>/db.lck`). Under `#[cfg(test)]` only: `Environment::with_paths(root, dbpath, cachedir, hookdir, repos: &[(&str, &str)], archs: &[&str])` (repos are name and `file://` URL; `SigLevel::NONE`; transactions use `TransFlag::NO_SCRIPTLET`) and `Environment::unreachable()` whose `open` panics.
  - `pub trait Aborter: Send + Sync { fn abort(&self); }` and `pub struct CancelGate` (`Clone`, shares state): `CancelGate::new(aborter: Arc<dyn Aborter>)`; `request_cancel(&self) -> CancelOutcome` with `pub enum CancelOutcome { Cancelled, TooLate }`; `checkpoint(&self) -> Result<(), HelperError>` (`Err` with `ErrorCode::Cancelled` once a cancel was accepted); `open_window(&self)` (call right before `trans_commit`); `commit_started(&self)` (call on the first package-operation event). States: planning (a cancel only sets a flag, honoured at the next `checkpoint`); window (a cancel calls `aborter.abort()` at once and records it); committed (a cancel returns `TooLate`).
  - `pub fn install(env: &Environment, packages: &[String], emit: Emit, gate: &CancelGate) -> Result<Vec<Change>, HelperError>`: resolve every name in the sync databases first (any unknown names fail the whole request with `NotFound` listing all of them and change nothing), add all to one transaction, prepare, `open_window`, commit, release. Maps libalpm callbacks to `Event::Progress` (refresh and resolve and download and verify and install stages), emitting a download event only when the integer percent for that package changes. Any libalpm question aborts the transaction with `Conflict` and a message listing the options. Errors map: db lock to `Locked` (message says the helper never removes it), file or package conflicts to `Conflict`, everything else to `Failed`; the transaction is always released.
- Fixture (`helper/src/fixture.rs`): `Fixture::new()` creates temp dirs for root, db, cache, hooks and repo; `publish(&self, name, version, depends: &[&str])` builds `<name>-<version>-any.pkg.tar.zst` with `bsdtar --zstd` (a `.PKGINFO` and `usr/bin/<name>`), runs `repo-add`, and replaces older versions; `env(&self) -> Environment`; `installed(&self) -> Vec<String>` (`name-version` from the local db); `lock_exists(&self) -> bool`; `file_exists(&self, path) -> bool`. It panics with a message naming the missing tool if `bsdtar`, `zstd` or `repo-add` is absent. The spike showed these steps work unprivileged and produce `refresh`, `resolve`, `download`, `verify` and `install` callbacks.

- [ ] **Step 1: Write the failing tests** (`#[cfg(test)]` in `gate.rs`, `install.rs`):
  - `gate_before_window_defers_and_checkpoint_fails`: `request_cancel` returns `Cancelled`, the recording aborter was **not** called, `checkpoint()` returns `Err` with code `Cancelled`.
  - `gate_in_window_aborts_immediately`: after `open_window`, `request_cancel` returns `Cancelled` and the aborter was called once.
  - `gate_after_commit_started_is_too_late`: `open_window`, `commit_started`, then `request_cancel` returns `TooLate`, no abort, `checkpoint()` is `Ok`.
  - `install_resolves_dependencies_and_writes_files`: publish `libfoo 1.0-1` and `app 1.0-1` depending on `libfoo`; `install(["app"])` returns changes for both, `installed()` is `["app-1.0-1","libfoo-1.0-1"]`, `file_exists("usr/bin/app")`, no lock left, and the emitted events include a `Download` and an `Install` progress event.
  - `install_unknown_name_is_not_found_and_changes_nothing` *(Review Focus 2 and 3)*: `["app","ghost","phantom"]` returns `NotFound` whose message contains both `ghost` and `phantom`; `installed()` is empty.
  - `install_conflict_leaves_the_database_untouched` *(Review Focus 2)*: publish `a 1.0-1` and `b 1.0-1` with a `conflict = a` line (extend `publish` with a `conflicts` argument or a second method); install `a`, then `install(["b"])` returns `Conflict`; `installed()` is still `["a-1.0-1"]` and `lock_exists()` is false.
  - `install_reports_a_stale_lock`: create `db.lck` by hand; `install` returns `Locked`; the file still exists afterwards (the helper never deletes it).
  - `download_events_are_throttled`: the number of `Download` events for one package is at most 101.
  - `cancel_before_start_changes_nothing` *(Review Focus 4)*: `gate.request_cancel()` then `install(["app"])` returns `Cancelled` and `installed()` is empty.
- [ ] **Step 2: Run** `cargo test -p store-helper` — expected: FAIL.
- [ ] **Step 3: Implement** the interfaces above. The mapping from libalpm callbacks to `Stage` is: database retrieve events to `Refresh`; resolve-deps events to `Resolve`; package retrieve events and the download callback to `Download`; integrity, keyring, load and file-conflict events to `Verify`; the progress callback's add and upgrade and remove starts to `Install`, `Upgrade` and `Remove` with the percent it reports. The first package-operation event calls `gate.commit_started()`.
- [ ] **Step 4: Run** `cargo test -p store-helper` — expected: all pass.
- [ ] **Step 5: Commit** `helper: add the environment, cancel gate and install operation`.

---

### Task 3: `remove`, `upgrade` and the protected set

**Files:**
- Create: `helper/src/remove.rs`, `helper/src/upgrade.rs`, `helper/src/protect.rs`
- Modify: `helper/src/lib.rs`, `helper/src/fixture.rs` (support seeding the local db with installed packages, including a `base` meta package)

**Interfaces:**
- Consumes: Task 2's `Environment`, `CancelGate`, `Emit`, `HelperError`, `Fixture`.
- Produces:
  - `pub fn remove(env: &Environment, packages: &[String], emit: Emit, gate: &CancelGate) -> Result<Vec<Change>, HelperError>`: every name must be installed (`NotFound` otherwise, listing all missing); the protected set is checked **before** libalpm sees the transaction (`Protected`, naming the offenders); one transaction, no cascade (a package others depend on fails with `DependencyBroken` listing the dependents); the same cancel gate and progress mapping as `install`.
  - `pub fn upgrade(env: &Environment, emit: Emit, gate: &CancelGate) -> Result<Vec<Change>, HelperError>`: refresh the sync databases then `sync_sysupgrade(false)` as **one** operation (the only place the databases are refreshed); `checkpoint()` before the refresh, before preparing and before the window; nothing to upgrade returns `Ok(vec![])`; changes have kind `Upgraded` with the new version.
  - `pub fn protected_packages(alpm: &Alpm) -> HashSet<String>`: `pacman`, `base`, `linux-store`, `linux-store-helper`, plus the transitive dependency closure of the installed `base` package (resolving each dependency to the installed package that provides it).

- [ ] **Step 1: Write the failing tests**:
  - `remove_deletes_files_and_keeps_shared_dependencies`: install `app` (needs `libfoo`), remove `["app"]`: `installed()` is `["libfoo-1.0-1"]`, `usr/bin/app` is gone.
  - `remove_with_dependents_is_refused` *(Review Focus 2)*: installed `app` needs `libfoo`; `remove(["libfoo"])` returns `DependencyBroken` mentioning `app`; `installed()` unchanged.
  - `remove_unknown_name_is_not_found`.
  - `remove_protected_is_refused_before_libalpm` *(Review Focus 3)*: seed `base` depending on `coreutils-fix`; `remove(["pacman"])`, `remove(["base"])`, `remove(["coreutils-fix"])`, `remove(["linux-store"])` each return `Protected`; `installed()` unchanged; the offender is named in the message.
  - `protected_set_includes_base_closure`: `protected_packages` contains `coreutils-fix` and the four fixed names.
  - `upgrade_refreshes_and_installs_newer_versions`: install `hello 1.0-1`, publish `hello 2.0-1`, `upgrade` returns one `Upgraded` change with version `2.0-1`, the file on disk reflects 2.0.
  - `upgrade_with_nothing_to_do_is_empty`.
  - `upgrade_cancelled_before_refresh_leaves_databases_alone`: cancel first; `upgrade` returns `Cancelled`; the sync database file is byte-identical to before.
- [ ] **Step 2: Run** `cargo test -p store-helper` — expected: FAIL.
- [ ] **Step 3: Implement** `remove`, `upgrade` and `protect`. Dependency-broken detection comes from libalpm's prepare error; do not reimplement it.
- [ ] **Step 4: Run** `cargo test -p store-helper` — expected: all pass.
- [ ] **Step 5: Commit** `helper: add remove, upgrade and the protected package set`.

---

### Task 4: `serve`, the binary, the abort path and the audit log

**Files:**
- Create: `helper/src/serve.rs`, `helper/src/audit.rs`, `helper/src/abort.rs`
- Modify: `helper/src/lib.rs`, `helper/src/main.rs`, `helper/Cargo.toml` (add `libc`, `signal-hook`)

**Interfaces:**
- Consumes: `Request`, `Event` and exit codes from Task 1; `install`, `remove`, `upgrade`, `Environment`, `CancelGate`, `Aborter` from Tasks 2 and 3.
- Produces:
  - `pub fn serve(env: &Environment, input: impl BufRead + Send + 'static, output: impl Write + Send + 'static, gate: CancelGate) -> i32`: reads **one** line (at most `MAX_REQUEST_BYTES`), `Request::parse`; on any parse error or a leading `Cancel` it writes `Event::Error{InvalidRequest}` and returns `EXIT_INVALID` **before** calling `env.open`. Otherwise it writes `Started`, spawns a reader thread that treats a later `Cancel` line, end of input, or any other later line as: valid `Cancel` and end of input call `gate.request_cancel()` (a `TooLate` answer is written as `Event::CancelIgnored`), other lines are ignored. It runs the operation, then writes `Done` and returns `EXIT_OK`, or writes `Error` and returns `EXIT_CANCELLED` (code `Cancelled`), `EXIT_INVALID` (`InvalidRequest`) or `EXIT_FAILED`. Every event is one `write_all` of a full line followed by a flush, under a mutex shared with the abort path.
  - `pub fn audit_line(caller_uid: Option<&str>, request: &Request) -> String` (for example `uid=1000 op=install packages=tree,sl`; the uid is shown only if it is all digits, else `unknown`) and `pub fn audit(line: &str)` which writes to the system log with `libc::syslog` under the identity `linux-store-helper`. `serve` calls it with `PKEXEC_UID` once the request is valid.
  - `pub struct ProcessAborter { lock_path: PathBuf }` implementing `Aborter`: removes **only** `lock_path`, writes `Event::Error{Cancelled}` to stdout and calls `std::process::exit(EXIT_CANCELLED)`. `pub fn ensure_root(euid: u32) -> Result<(), HelperError>` (`Failed`, message "run through pkexec").
  - `main.rs`: `ensure_root(libc::geteuid())`, `Environment::system()`, a `CancelGate` with `ProcessAborter`, a `signal-hook` thread that turns `SIGTERM`, `SIGINT` and `SIGHUP` into `gate.request_cancel()`, then `std::process::exit(serve(...))`. Any setup error is printed as an `Event::Error` line and exits with `EXIT_FAILED`.

- [ ] **Step 1: Write the failing tests** (in `serve.rs` and `audit.rs`). End of input means "cancel" (the GUI went away), so tests that must **not** be cancelled use a test helper `HeldInput`: a `BufRead` that yields the request line and then blocks on an `mpsc` channel until the test drops the sender after `serve` returns (`serve` never joins the reader thread, so it can return while the reader still waits). Only `end_of_input_after_the_request_is_a_cancel_request` uses a plain `Cursor`. Output is collected through a shared buffer.
  - `hostile_requests_never_open_libalpm` *(Review Focus 1)*: with `Environment::unreachable()`, each of the Task 1 hostile lines, plus a leading `{"op":"cancel"}`, returns `EXIT_INVALID`, the output is one `error` event with `invalid_request`, and nothing panics.
  - `install_end_to_end`: fixture `app` and `libfoo`; input (held open) `{"op":"install","packages":["app"]}\n`; the output starts with `started`, includes `progress`, ends with `done` listing both packages; returns `EXIT_OK`.
  - `unknown_package_returns_failed_exit_and_error_event`: exit `EXIT_FAILED`, last event `error` with `not_found`.
  - `cancel_before_work_is_exit_cancelled`: a gate already cancelled before `serve` starts (held-open input): returns `EXIT_CANCELLED`, nothing installed.
  - `end_of_input_after_the_request_is_a_cancel_request`: documented behaviour; the operation either completes or is cancelled, and in both cases leaves a consistent database and no lock.
  - `late_cancel_is_reported_as_cancel_ignored`: drive the gate to the committed state, send a cancel, expect `cancel_ignored` in the output and the operation to finish with `done`.
  - `audit_line_formats_and_sanitises`: `Some("1000")`, `None`, `Some("12x")` and `Some("")` give `uid=1000`, `uid=unknown` (twice more); a `Request::Install` lists its names comma-separated; `Upgrade` has no packages.
  - `ensure_root_rejects_non_root`: `ensure_root(1000)` is `Err`, `ensure_root(0)` is `Ok`.
  - `process_aborter_removes_only_its_own_lock`: temp dir with `db.lck` and `other.lck`; `ProcessAborter::cleanup_only()` (a `pub(crate)` method that does everything except exit and the event write) removes only `db.lck`.
- [ ] **Step 2: Run** `cargo test -p store-helper` — expected: FAIL.
- [ ] **Step 3: Implement** `serve`, `audit`, `abort` and `main.rs`.
- [ ] **Step 4: Run** `cargo test -p store-helper`, then `cargo build --release -p store-helper` and `target/release/store-helper < /dev/null; echo $?` as a **normal user** — expected: all tests pass, and the binary prints one `error` event saying it must run through `pkexec` and exits `1` (it refuses to touch anything as a non-root user).
- [ ] **Step 5: Commit** `helper: add the request loop, abort path, audit log and binary`.

---

### Task 5: `Installer` trait and the `pkexec` client

**Files:**
- Modify: `Cargo.toml` (add `"installers/pkexec"`), `core/Cargo.toml` (depend on `store-protocol`), `core/src/lib.rs`, `core/src/testing.rs`
- Create: `core/src/installer.rs`, `installers/pkexec/Cargo.toml` (package `store-installer-pkexec`), `installers/pkexec/src/lib.rs`, `installers/pkexec/tests/client.rs`

**Interfaces:**
- Consumes: `Request`, `Event`, `ErrorCode` from `store-protocol`.
- Produces:
  - In `store_core`: `pub struct CancelToken` (`Clone`, `Default`; `cancel(&self)`, `is_cancelled(&self) -> bool`); `pub enum InstallError { Rejected { code: ErrorCode, message: String }, Dismissed, NotAuthorized(String), Spawn(std::io::Error), Protocol(String), Failed { exit_code: Option<i32>, stderr: String } }` (`thiserror`); `pub trait Installer: Send + Sync { fn run(&self, request: &Request, cancel: &CancelToken, on_event: &mut dyn FnMut(Event)) -> Result<(), InstallError>; }`; under the `testing` feature `FakeInstaller::new(events: Vec<Event>, result: Result<(), InstallError>)` (records the requests it receives).
  - In `store_installer_pkexec`: `pub const HELPER_PATH: &str = "/usr/lib/linux-store/store-helper";` `pub struct PkexecInstaller`; `PkexecInstaller::system()` (command `["pkexec", HELPER_PATH]`); `PkexecInstaller::with_command(command: Vec<OsString>)` (first element is the program; used by tests). `run`: spawn with piped stdin, stdout and stderr; write `request.to_line()`; a watcher thread writes `{"op":"cancel"}\n` when the token is set; read stdout line by line with a length cap, parsing each with `Event::parse`; capture at most 8 KiB of stderr; when stdout ends, wait for the exit status. Results: exit 0 with a `done` event is `Ok`; an `error` event becomes `Rejected{code, message}`; exit 126 is `Dismissed`; exit 127 is `NotAuthorized(stderr)`; a line that does not parse is `Protocol` (stdin is closed, which the helper reads as a cancel request); a spawn error is `Spawn`; any other non-zero exit without an `error` event is `Failed{..}`. Dropping or finishing a run closes stdin and waits at most 5 seconds before giving up on the child (a root child cannot be signalled by the user).

- [ ] **Step 1: Write the failing tests** in `installers/pkexec/tests/client.rs`, using small `sh` scripts written to a temp dir as the "helper":
  - `delivers_events_in_order_and_succeeds`: the script prints a `started`, a `progress` and a `done` line and exits 0; `on_event` receives them in order; `run` is `Ok`.
  - `writes_the_request_as_one_json_line`: the script saves its first stdin line to a file; it equals `request.to_line()`.
  - `error_event_becomes_rejected`: prints `{"event":"error","code":"not_found","message":"ghost"}`, exits 1; result is `Rejected{code: NotFound, message: "ghost"}`.
  - `exit_126_is_dismissed_and_127_is_not_authorized`: two scripts; the second writes `denied` to stderr and the error carries it.
  - `garbage_output_is_a_protocol_error_not_a_panic` *(Review Focus 5)*: prints `not json`, then sleeps; `run` returns `Protocol` within 6 seconds.
  - `silent_exit_is_a_failure_not_a_hang`: exits 3 with no output; `Failed{exit_code: Some(3), ..}`.
  - `missing_program_is_a_spawn_error`.
  - `cancel_token_sends_a_cancel_line`: the script reads lines and, on `{"op":"cancel"}`, prints an `error` event with code `cancelled` and exits 3; with the token set the result is `Rejected{code: Cancelled, ..}`.
  - `oversized_output_line_is_a_protocol_error`: a 200 KiB line.
  - In `core`: `fake_installer_records_requests_and_replays_events`.
- [ ] **Step 2: Run** `cargo test -p store-installer-pkexec -p store-core --features store-core/testing` — expected: FAIL.
- [ ] **Step 3: Implement** the trait, the token, the fake and the client.
- [ ] **Step 4: Run** the full test command — expected: all pass.
- [ ] **Step 5: Commit** `installers: add the Installer trait and the pkexec client`.

---

### Task 6: Polkit policy, CLI commands, docs and the manual check

**Files:**
- Create: `packaging/org.linuxstore.helper.policy`, `packaging/dev-install.sh`, `packaging/dev-uninstall.sh`, `packaging/manual-check.sh`, `installers/pkexec/tests/policy.rs`
- Modify: `cli/Cargo.toml`, `cli/src/lib.rs`, `cli/src/main.rs`, `cli/tests/cli.rs`, `README.md`, `AGENTS.md`

**Interfaces:**
- Consumes: `PkexecInstaller::system()`, `HELPER_PATH`, `Installer`, `FakeInstaller`, `Request`, `Event`, `InstallError`.
- Produces:
  - The policy file: one action `org.linuxstore.helper.run` with `allow_any` `auth_admin`, `allow_inactive` `auth_admin`, `allow_active` `auth_admin_keep`, a clear message, and `<annotate key="org.freedesktop.policykit.exec.path">` equal to `HELPER_PATH`.
  - `store_cli::run(args: &[String], make_store: impl FnOnce(&Path) -> store_core::Result<Store>, installer: &dyn Installer, out: &mut W, err: &mut E) -> i32` (the new `installer` parameter is the only signature change). New commands: `store install <names>...`, `store remove <names>...`, `store upgrade`. Output: `started`, then one `[stage] package percent%` line only when the percent changes, then a final line listing the changes. Exit codes: 0 success; 126 when the user dismissed the prompt; 127 when not authorized; 1 for every other error (the message goes to stderr). Names are validated with `is_valid_name` **before** the helper is started, so a bad name never costs a password prompt.
  - `packaging/dev-install.sh` (run by the user with `sudo`): installs `target/release/store-helper` to `/usr/lib/linux-store/store-helper` (`root:root`, `0755`) and the policy to `/usr/share/polkit-1/actions/` (`0644`); `dev-uninstall.sh` removes both. `packaging/manual-check.sh` prints the steps and runs `store install tree`, `store install cowsay`, `store remove tree cowsay`, telling the user which of them should prompt and which should not.

- [ ] **Step 1: Write the failing tests**:
  - `policy.rs`: parse the policy with `quick-xml` and assert the action id, the three defaults and that the `exec.path` annotation equals `store_installer_pkexec::HELPER_PATH`.
  - `cli/tests/cli.rs` with a `FakeInstaller`: `install_sends_the_request_and_prints_progress`; `remove_and_upgrade_send_the_matching_requests`; `invalid_name_is_rejected_before_the_helper_runs` (the fake records zero requests, exit 1, stderr names the problem); `dismissed_exits_126`; `not_authorized_exits_127`; `rejected_error_prints_the_message_and_exits_1`; `progress_lines_are_throttled` (100 progress events at the same percent produce one line).
  - The existing CLI tests are updated mechanically to pass a `FakeInstaller` (their helper function builds it).
- [ ] **Step 2: Run** the full test command — expected: FAIL.
- [ ] **Step 3: Implement** the policy, the scripts, the CLI changes and `main.rs` (builds `PkexecInstaller::system()`); update the README layout table and `AGENTS.md` (layout, the "never run `pkexec`, `sudo` or the real helper from an agent" rule, how to run `dev-install.sh` and `manual-check.sh`).
- [ ] **Step 4: Run** the full test command, then `sh -n` on each script — expected: all pass, no syntax errors. Confirm CI still passes on the Arch container (the fixture needs `bsdtar`, `zstd`, `repo-add`; if the container lacks `libarchive` or `zstd`, add them to the `pacman -S` line of `.github/workflows/ci.yml` and mention it in the commit).
- [ ] **Step 5: Commit** `cli: install, remove and upgrade through the pkexec helper`, and report that the **manual end-to-end check is the user's to run** (`sudo sh packaging/dev-install.sh`, then `sh packaging/manual-check.sh`, then `sudo sh packaging/dev-uninstall.sh`), with the three things to report back: whether the graphical prompt appeared, whether the second command in the same session asked again, and whether `pkexec` runs the helper as a child or replaces itself.

---

## Self-Review notes

- **Spec coverage:** install, remove and upgrade in single libalpm transactions (Tasks 2 and 3); names-only validated requests (Task 1); polkit action with `auth_admin_keep` and fixed `exec.path` (Task 6); protocol, events, exit codes (Tasks 1 and 4); cancel before the commit window and `too_late` after (Tasks 2 and 4); journal audit (Task 4); `DownloadUser` sandbox through `alpm_with_conf` (Task 2); the `Installer` trait (Task 5); test seams only under `#[cfg(test)]` (Task 2). Not covered on purpose: installing AUR-built package files, the D-Bus mode, the PackageKit adapter.
- **Changes from the design, decided here:** `EXIT_CANCELLED = 3` is new (the design listed 0, 1, 2); `CancelIgnored` is a fifth event; cancel inside the commit window is implemented by removing the helper's own `db.lck` and exiting (the `alpm` crate exposes neither the handle pointer nor `trans_interrupt`), which is safe only before the first package operation, hence the gate; the design's "sync only inside upgrade" is enforced by having no refresh operation.
- **Type consistency:** `HelperError`/`Emit`/`CancelGate` are defined in Task 2 and used unchanged in Tasks 3 and 4; `Installer`, `CancelToken` and `InstallError` are defined in Task 5 and used in Task 6; the JSON shapes are pinned in Task 1.
- **Known limits:** the real `pkexec` behaviours (whether it replaces itself, the exact "no agent" message, the five-minute cache across calls) are only checked by the manual script; the graphical agent on the dev machine appears on demand.
