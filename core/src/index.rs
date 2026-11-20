use std::path::Path;

use rusqlite::{Connection, Row, params};

use crate::{Error, Package, Result, SourceId};

const SCHEMA_VERSION: i64 = 2;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS packages (
        id INTEGER PRIMARY KEY,
        source TEXT NOT NULL,
        name TEXT NOT NULL,
        version TEXT NOT NULL,
        installed_version TEXT,
        title TEXT,
        description TEXT,
        repo TEXT,
        url TEXT,
        download_size INTEGER NOT NULL,
        installed_size INTEGER NOT NULL,
        UNIQUE (source, name)
    );
    CREATE VIRTUAL TABLE IF NOT EXISTS packages_fts
        USING fts5(name, title, description, tokenize = 'unicode61');
";

/// Columns of `packages`, in the order `row_to_package` reads them.
const COLUMNS: &str = "source, name, version, installed_version, title, description, repo, url, \
                       download_size, installed_size";

/// Local SQLite index of every package known to every source.
pub struct Index {
    conn: Connection,
}

impl Index {
    /// Opens (creating if missing) the index at `path`. The parent directory must exist.
    pub fn open(path: &Path) -> Result<Index> {
        let conn = Connection::open(path)?;
        conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get::<_, String>(0))?;
        Index::init(conn)
    }

    pub fn open_in_memory() -> Result<Index> {
        Index::init(Connection::open_in_memory()?)
    }

    /// Creates the schema for a brand-new database and rebuilds an older one (the index is only
    /// a cache). Opening an up-to-date index must not write, otherwise a reader (a search)
    /// would wait for any concurrent refresh.
    fn init(conn: Connection) -> Result<Index> {
        let found: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if found > SCHEMA_VERSION {
            return Err(Error::UnsupportedIndexVersion {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        if found != SCHEMA_VERSION {
            // Older schema: drop it; the next refresh refills the cache.
            conn.execute_batch(
                "DROP TABLE IF EXISTS packages_fts; DROP TABLE IF EXISTS packages;",
            )?;
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(Index { conn })
    }

    /// Atomically replaces everything stored for `source` with `packages`.
    pub fn replace_source(&mut self, source: &SourceId, packages: &[Package]) -> Result<()> {
        if let Some(bad) = packages.iter().find(|p| &p.source != source) {
            return Err(Error::SourceMismatch {
                expected: source.clone(),
                found: bad.source.clone(),
            });
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM packages_fts WHERE rowid IN (SELECT id FROM packages WHERE source = ?1)",
            [source.as_str()],
        )?;
        tx.execute("DELETE FROM packages WHERE source = ?1", [source.as_str()])?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO packages (source, name, version, installed_version, title, description, \
                 repo, url, download_size, installed_size) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            let mut insert_fts = tx.prepare(
                "INSERT INTO packages_fts (rowid, name, title, description) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for p in packages {
                insert.execute(params![
                    p.source.as_str(),
                    p.name,
                    p.version,
                    p.installed_version,
                    p.title,
                    p.description,
                    p.repo,
                    p.url,
                    to_sql_size(p.download_size),
                    to_sql_size(p.installed_size),
                ])?;
                let id = tx.last_insert_rowid();
                insert_fts.execute(params![
                    id,
                    p.name,
                    p.title.as_deref().unwrap_or(""),
                    p.description.as_deref().unwrap_or("")
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get(&self, source: &SourceId, name: &str) -> Result<Option<Package>> {
        let sql = format!("SELECT {COLUMNS} FROM packages WHERE source = ?1 AND name = ?2");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map(params![source.as_str(), name], row_to_package)?;
        Ok(rows.next().transpose()?)
    }

    /// Installed packages across all sources, sorted by name then source.
    pub fn installed(&self) -> Result<Vec<Package>> {
        let sql = format!(
            "SELECT {COLUMNS} FROM packages WHERE installed_version IS NOT NULL ORDER BY name, source"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], row_to_package)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Full-text search over name and description.
    ///
    /// The query is split into alphanumeric terms, each passed to FTS5 as a quoted literal (so
    /// user input can never be interpreted as FTS syntax); the last term matches as a prefix and
    /// all terms must match. Ordering: exact name, then name starting with the query, then
    /// bm25 (name and title weighted 10x over description), then shorter name, then name.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Package>> {
        let terms: Vec<&str> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let last = terms.len() - 1;
        let fts_query = terms
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if i == last {
                    format!("\"{t}\"*")
                } else {
                    format!("\"{t}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let columns = COLUMNS
            .split(", ")
            .map(|c| format!("p.{}", c.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {columns} FROM packages_fts \
             JOIN packages p ON p.id = packages_fts.rowid \
             WHERE packages_fts MATCH ?1 \
             ORDER BY lower(p.name) = lower(?2) DESC, \
                      substr(lower(p.name), 1, length(?2)) = lower(?2) DESC, \
                      bm25(packages_fts, 10.0, 10.0, 1.0), length(p.name), p.name \
             LIMIT ?3"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![
                fts_query,
                query.trim(),
                i64::try_from(limit).unwrap_or(i64::MAX)
            ],
            row_to_package,
        )?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM packages", [], |r| r.get(0))?;
        Ok(n as usize)
    }
}

fn to_sql_size(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn row_to_package(row: &Row<'_>) -> rusqlite::Result<Package> {
    Ok(Package {
        source: SourceId::new(row.get::<_, String>(0)?),
        name: row.get(1)?,
        version: row.get(2)?,
        installed_version: row.get(3)?,
        title: row.get(4)?,
        description: row.get(5)?,
        repo: row.get(6)?,
        url: row.get(7)?,
        download_size: row.get::<_, i64>(8)?.max(0) as u64,
        installed_size: row.get::<_, i64>(9)?.max(0) as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::pkg;
    use crate::{Error, Package, SourceId};

    fn sid(s: &str) -> SourceId {
        SourceId::new(s)
    }

    fn full(source: &str, name: &str) -> Package {
        Package {
            source: sid(source),
            name: name.into(),
            version: "2.0-1".into(),
            installed_version: Some("1.0-1".into()),
            title: Some("A Tool".into()),
            description: Some("A tool".into()),
            repo: Some("extra".into()),
            url: Some("https://example.org".into()),
            download_size: 1234,
            installed_size: 5678,
        }
    }

    fn installed(source: &str, name: &str) -> Package {
        Package {
            installed_version: Some("1.0".into()),
            ..pkg(source, name, "x")
        }
    }

    #[test]
    fn roundtrip_preserves_every_field() {
        let mut idx = Index::open_in_memory().unwrap();
        let bare = Package {
            title: None,
            description: None,
            repo: None,
            url: None,
            installed_version: None,
            ..full("a", "bare")
        };
        let rich = full("a", "rich");
        idx.replace_source(&sid("a"), &[rich.clone(), bare.clone()])
            .unwrap();
        assert_eq!(idx.get(&sid("a"), "rich").unwrap(), Some(rich));
        assert_eq!(idx.get(&sid("a"), "bare").unwrap(), Some(bare));
        assert_eq!(idx.get(&sid("a"), "missing").unwrap(), None);
    }

    #[test]
    fn replace_source_replaces_only_that_source() {
        let mut idx = Index::open_in_memory().unwrap();
        idx.replace_source(&sid("a"), &[pkg("a", "x", "d")])
            .unwrap();
        idx.replace_source(&sid("b"), &[pkg("b", "y", "d")])
            .unwrap();
        idx.replace_source(&sid("a"), &[pkg("a", "z", "d")])
            .unwrap();
        assert_eq!(idx.get(&sid("a"), "x").unwrap(), None);
        assert!(idx.get(&sid("a"), "z").unwrap().is_some());
        assert!(idx.get(&sid("b"), "y").unwrap().is_some());
        assert_eq!(idx.count().unwrap(), 2);
    }

    #[test]
    fn replace_source_is_atomic_on_mismatch() {
        let mut idx = Index::open_in_memory().unwrap();
        idx.replace_source(&sid("a"), &[pkg("a", "x", "d")])
            .unwrap();
        let err = idx
            .replace_source(&sid("a"), &[pkg("a", "ok", "d"), pkg("b", "bad", "d")])
            .unwrap_err();
        assert!(matches!(err, Error::SourceMismatch { .. }));
        assert!(idx.get(&sid("a"), "x").unwrap().is_some());
        assert_eq!(idx.get(&sid("a"), "ok").unwrap(), None);
    }

    #[test]
    fn replace_source_rolls_back_when_an_insert_fails() {
        let mut idx = Index::open_in_memory().unwrap();
        idx.replace_source(&sid("a"), &[pkg("a", "x", "d")])
            .unwrap();
        // Duplicate name violates UNIQUE(source, name) after the old rows were deleted.
        let err = idx
            .replace_source(&sid("a"), &[pkg("a", "dup", "d"), pkg("a", "dup", "d")])
            .unwrap_err();
        assert!(matches!(err, Error::Index(_)));
        assert!(idx.get(&sid("a"), "x").unwrap().is_some());
        assert_eq!(idx.count().unwrap(), 1);
    }

    #[test]
    fn installed_lists_only_installed_sorted() {
        let mut idx = Index::open_in_memory().unwrap();
        idx.replace_source(
            &sid("a"),
            &[
                installed("a", "zsh"),
                pkg("a", "git", "d"),
                installed("a", "bash"),
            ],
        )
        .unwrap();
        let names: Vec<String> = idx
            .installed()
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["bash", "zsh"]);
    }

    #[test]
    fn index_persists_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let mut idx = Index::open(&path).unwrap();
            idx.replace_source(&sid("a"), &[pkg("a", "x", "d"), pkg("a", "y", "d")])
                .unwrap();
        }
        assert_eq!(Index::open(&path).unwrap().count().unwrap(), 2);
    }

    fn indexed(packages: &[Package]) -> Index {
        let mut idx = Index::open_in_memory().unwrap();
        idx.replace_source(&sid("a"), packages).unwrap();
        idx
    }

    fn names(found: Vec<Package>) -> Vec<String> {
        found.into_iter().map(|p| p.name).collect()
    }

    #[test]
    fn search_finds_by_name_prefix() {
        let idx = indexed(&[
            pkg("a", "firefox", "browser"),
            pkg("a", "firefox-i18n-fr", "french"),
            pkg("a", "git", "vcs"),
        ]);
        let mut found = names(idx.search("fire", 10).unwrap());
        found.sort();
        assert_eq!(found, ["firefox", "firefox-i18n-fr"]);
    }

    #[test]
    fn exact_name_ranks_first() {
        let idx = indexed(&[
            pkg("a", "firefox-i18n-fr", "french"),
            pkg("a", "firefox-developer-edition", "dev"),
            pkg("a", "firefox", "browser"),
        ]);
        assert_eq!(idx.search("firefox", 10).unwrap()[0].name, "firefox");
    }

    fn long_description() -> String {
        vec!["word"; 60].join(" ")
    }

    #[test]
    fn exact_name_beats_a_stronger_text_match() {
        // bm25 normalises by document length, so on its own the short competitor would win.
        let idx = indexed(&[
            pkg("a", "firefox-nightly", "firefox"),
            pkg("a", "firefox", &long_description()),
        ]);
        assert_eq!(idx.search("firefox", 10).unwrap()[0].name, "firefox");
    }

    #[test]
    fn name_prefix_beats_a_stronger_text_match() {
        let idx = indexed(&[
            pkg("a", "my-git-thing", "git"),
            pkg("a", "git-lfs", &long_description()),
        ]);
        assert_eq!(idx.search("git", 10).unwrap()[0].name, "git-lfs");
    }

    #[test]
    fn search_matches_description() {
        let idx = indexed(&[pkg(
            "a",
            "git",
            "the fast distributed version control system",
        )]);
        assert_eq!(names(idx.search("distributed", 10).unwrap()), ["git"]);
    }

    #[test]
    fn search_requires_all_terms() {
        let idx = indexed(&[pkg(
            "a",
            "git",
            "the fast distributed version control system",
        )]);
        assert_eq!(names(idx.search("fast distributed", 10).unwrap()), ["git"]);
        assert!(idx.search("fast banana", 10).unwrap().is_empty());
    }

    #[test]
    fn search_is_case_and_accent_insensitive() {
        let idx = indexed(&[pkg("a", "editor-pt", "Editor de ação")]);
        assert_eq!(names(idx.search("ACAO", 10).unwrap()), ["editor-pt"]);
    }

    #[test]
    fn search_survives_fts_syntax() {
        let idx = indexed(&[pkg("a", "firefox", "browser"), pkg("a", "c", "compiler")]);
        for q in [
            "\"",
            "firefox\"",
            "a AND",
            "(",
            "foo:bar",
            "-x",
            "*",
            "NEAR(",
            "c++",
        ] {
            assert!(idx.search(q, 10).is_ok(), "query {q:?} errored");
        }
    }

    #[test]
    fn empty_query_returns_nothing() {
        let idx = indexed(&[pkg("a", "firefox", "browser")]);
        assert!(idx.search("", 10).unwrap().is_empty());
        assert!(idx.search("   ", 10).unwrap().is_empty());
    }

    #[test]
    fn search_respects_limit() {
        let many: Vec<Package> = (0..20)
            .map(|i| pkg("a", &format!("tool-{i}"), "tool"))
            .collect();
        let idx = indexed(&many);
        assert_eq!(idx.search("tool", 5).unwrap().len(), 5);
    }

    #[test]
    fn opening_an_existing_index_does_not_need_the_write_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let mut idx = Index::open(&path).unwrap();
            idx.replace_source(&sid("a"), &[pkg("a", "firefox", "browser")])
                .unwrap();
        }
        // Another process (a refresh) holds the write lock.
        let writer = rusqlite::Connection::open(&path).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        let started = std::time::Instant::now();
        let reader = Index::open(&path).expect("open must not block on the writer");
        assert_eq!(names(reader.search("firefox", 10).unwrap()), ["firefox"]);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn opening_a_newer_index_is_refused_and_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        drop(Index::open(&path).unwrap());
        rusqlite::Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        let err = Index::open(&path)
            .err()
            .expect("a newer schema must be refused");
        assert!(
            matches!(err, Error::UnsupportedIndexVersion { found: 99, .. }),
            "{err:?}"
        );
        let version: i64 = rusqlite::Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 99);
    }

    #[test]
    fn title_roundtrips() {
        let mut idx = Index::open_in_memory().unwrap();
        let p = Package {
            title: Some("Firefox".into()),
            ..pkg("a", "org.mozilla.firefox", "browser")
        };
        idx.replace_source(&sid("a"), &[p.clone()]).unwrap();
        assert_eq!(idx.get(&sid("a"), "org.mozilla.firefox").unwrap(), Some(p));
    }

    #[test]
    fn search_finds_by_title() {
        let idx = indexed(&[Package {
            title: Some("Firefox".into()),
            ..pkg("a", "org.mozilla.firefox", "web browser")
        }]);
        assert_eq!(
            names(idx.search("firefox", 10).unwrap()),
            ["org.mozilla.firefox"]
        );
        assert_eq!(
            names(idx.search("org.mozilla", 10).unwrap()),
            ["org.mozilla.firefox"]
        );
    }

    #[test]
    fn schema_1_index_is_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE packages (id INTEGER PRIMARY KEY, source TEXT NOT NULL,
                     name TEXT NOT NULL, version TEXT NOT NULL, installed_version TEXT,
                     description TEXT, repo TEXT, url TEXT, download_size INTEGER NOT NULL,
                     installed_size INTEGER NOT NULL, UNIQUE (source, name));
                 CREATE VIRTUAL TABLE packages_fts USING fts5(name, description);
                 INSERT INTO packages (source, name, version, download_size, installed_size)
                     VALUES ('a', 'old', '1', 0, 0);
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        }
        let mut idx = Index::open(&path).expect("a schema 1 index must be rebuilt, not refused");
        assert_eq!(idx.count().unwrap(), 0);
        idx.replace_source(&sid("a"), &[pkg("a", "new", "d")])
            .unwrap();
        assert_eq!(names(idx.search("new", 10).unwrap()), ["new"]);
        let version: i64 = rusqlite::Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2);
    }
}
