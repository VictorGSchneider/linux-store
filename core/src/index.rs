use std::path::Path;

use rusqlite::{Connection, Row, params};

use crate::{Error, Package, Result, SourceId};

const SCHEMA_VERSION: i64 = 4;

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
        app_id TEXT,
        categories TEXT NOT NULL DEFAULT '[]',
        icon TEXT,
        popularity REAL,
        upgradable INTEGER NOT NULL DEFAULT 0,
        UNIQUE (source, name)
    );
    CREATE INDEX IF NOT EXISTS packages_name_lower ON packages (lower(name));
    CREATE INDEX IF NOT EXISTS packages_title_lower ON packages (lower(title));
    CREATE INDEX IF NOT EXISTS packages_app_id ON packages (app_id);
    CREATE VIRTUAL TABLE IF NOT EXISTS packages_fts
        USING fts5(name, title, description, tokenize = 'unicode61');
    CREATE TABLE IF NOT EXISTS app_links (
        source TEXT NOT NULL,
        name TEXT NOT NULL,
        app_key TEXT NOT NULL,
        PRIMARY KEY (source, name)
    );
    CREATE INDEX IF NOT EXISTS app_links_key ON app_links (app_key);
";

/// Columns of `packages`, in the order `row_to_package` reads them.
const COLUMNS: &str = "source, name, version, installed_version, title, description, repo, url, \
                       download_size, installed_size, app_id, categories, icon, popularity, upgradable";

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
        let found = user_version(&conn)?;
        if found > SCHEMA_VERSION {
            return Err(Error::UnsupportedIndexVersion {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        if found != SCHEMA_VERSION {
            migrate(&conn)?;
        } else {
            ensure_lookup_indexes(&conn)?;
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
                 repo, url, download_size, installed_size, app_id, categories, icon, popularity, \
                 upgradable) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
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
                    p.app_id,
                    categories_to_sql(&p.categories),
                    p.icon,
                    p.popularity,
                    p.upgradable,
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
        rebuild_links_in(&tx)?;
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
    /// all terms must match. Ordering: exact name or title, then name starting with the query, then
    /// bm25 (name and title weighted 10x over description), then shorter name, then name. An exact
    /// title counts as an exact name.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Package>> {
        self.search_in(query, limit, None)
    }

    /// The sources that have at least one package indexed.
    pub fn sources(&self) -> Result<Vec<SourceId>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT source FROM packages ORDER BY source")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .map(|s| SourceId::new(&s))
            .collect())
    }

    /// Like [`Index::search`], optionally restricted to one source.
    pub fn search_in(
        &self,
        query: &str,
        limit: usize,
        source: Option<&SourceId>,
    ) -> Result<Vec<Package>> {
        let source = source.map(|s| s.to_string());
        let terms: Vec<&str> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        // One or two letters match tens of thousands of rows, and ranking them all by bm25 is
        // slow (about 90 ms for `a` over the AUR). Such a query is a name prefix, so answer it
        // from the name index instead. Non-ASCII terms keep the full-text path because SQLite's
        // `lower()` only folds ASCII.
        if let [term] = terms.as_slice()
            && term.len() <= 2
            && term.is_ascii()
        {
            return self.search_name_prefix(&term.to_ascii_lowercase(), limit, source.as_deref());
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
             WHERE packages_fts MATCH ?1 AND (?5 IS NULL OR p.source = ?5) \
             ORDER BY (lower(p.name) = lower(?2) OR lower(p.name) = ?4 \
                       OR lower(coalesce(p.title, '')) = lower(?2)) DESC, \
                      (substr(lower(p.name), 1, length(?2)) = lower(?2) \
                       OR substr(lower(p.name), 1, length(?4)) = ?4) DESC, \
                      bm25(packages_fts, 10.0, 10.0, 1.0), length(p.name), p.name \
             LIMIT ?3"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![
                fts_query,
                query.trim(),
                i64::try_from(limit).unwrap_or(i64::MAX),
                // "firefox developer edition" is the package firefox-developer-edition
                terms.join("-").to_lowercase(),
                source,
            ],
            row_to_package,
        )?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Packages whose lowercased name or title starts with `prefix` (ASCII, lowercase, non-empty):
    /// an exact name or title first, then shorter names, then by name.
    fn search_name_prefix(
        &self,
        prefix: &str,
        limit: usize,
        source: Option<&str>,
    ) -> Result<Vec<Package>> {
        let mut upper = prefix.to_string();
        let last = upper.pop().expect("prefix is not empty");
        upper.push(char::from(last as u8 + 1));
        let sql = format!(
            "SELECT {COLUMNS} FROM packages \
             WHERE ((lower(name) >= ?1 AND lower(name) < ?2) \
                OR (lower(title) >= ?1 AND lower(title) < ?2)) \
               AND (?4 IS NULL OR source = ?4) \
             ORDER BY (lower(name) = ?1 OR coalesce(lower(title), '') = ?1) DESC, \
                      length(coalesce(title, name)), name, source \
             LIMIT ?3"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![
                prefix,
                upper,
                i64::try_from(limit).unwrap_or(i64::MAX),
                source
            ],
            row_to_package,
        )?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Rebuilds `app_links` from `packages` (see [`rebuild_links_in`]).
    pub fn rebuild_links(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        rebuild_links_in(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// The app key of each package, in the same order. A package that is not indexed is an error.
    pub fn keys_of(&self, packages: &[Package]) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT app_key FROM app_links WHERE source = ?1 AND name = ?2")?;
        packages
            .iter()
            .map(|p| Ok(stmt.query_row(params![p.source.as_str(), p.name], |r| r.get(0))?))
            .collect()
    }

    /// Every package linked to `app_key`, ordered by source then name.
    pub fn members(&self, app_key: &str) -> Result<Vec<Package>> {
        let columns = prefixed_columns();
        let sql = format!(
            "SELECT {columns} FROM app_links l \
             JOIN packages p ON p.source = l.source AND p.name = l.name \
             WHERE l.app_key = ?1 ORDER BY p.source, p.name"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([app_key], row_to_package)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// How many packages each source has, ordered by source.
    pub fn source_counts(&self) -> Result<Vec<(SourceId, usize)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT source, COUNT(*) FROM packages GROUP BY source ORDER BY source")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .map(|(s, n)| (SourceId::new(&s), n as usize))
            .collect())
    }

    /// Packages worth showing on a landing page. The AUR is ordered by popularity (unknown
    /// last); any other source shows only packages with an icon, in an order that is pseudo-random
    /// but the same for the same `seed`. A source with no icons falls back to the first by name.
    pub fn featured(&self, source: &SourceId, limit: usize, seed: u64) -> Result<Vec<Package>> {
        let sql_limit = i64::try_from(limit).unwrap_or(i64::MAX);
        if source.as_str() == "aur" {
            let sql = format!(
                "SELECT {COLUMNS} FROM packages WHERE source = ?1 \
                 ORDER BY popularity IS NULL, popularity DESC, name LIMIT ?2"
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params![source.as_str(), sql_limit], row_to_package)?;
            return Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?);
        }
        let sql = format!("SELECT {COLUMNS} FROM packages WHERE source = ?1 AND icon IS NOT NULL");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut with_icon = stmt
            .query_map([source.as_str()], row_to_package)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if with_icon.is_empty() {
            let sql =
                format!("SELECT {COLUMNS} FROM packages WHERE source = ?1 ORDER BY name LIMIT ?2");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params![source.as_str(), sql_limit], row_to_package)?;
            return Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?);
        }
        with_icon.sort_by_cached_key(|p| (seeded_hash(seed, &p.name), p.name.clone()));
        with_icon.truncate(limit);
        Ok(with_icon)
    }

    /// Packages flagged as upgradable, sorted by name then source.
    pub fn upgradable(&self) -> Result<Vec<Package>> {
        let sql =
            format!("SELECT {COLUMNS} FROM packages WHERE upgradable = 1 ORDER BY name, source");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], row_to_package)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM packages", [], |r| r.get(0))?;
        Ok(n as usize)
    }
}

/// `COLUMNS` qualified with the alias `p`.
fn prefixed_columns() -> String {
    COLUMNS
        .split(", ")
        .map(|c| format!("p.{}", c.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Deletes and refills `app_links`, a pure function of `packages`, with one statement.
///
/// 1. base key: the package's own `app_id`; else the smallest `app_id` among packages with the
///    same lowercased name (any source); else its lowercased name;
/// 2. among packages of the same source and base, the smallest name keeps the base and each other
///    gets `base#name`, so one app never has two variants from the same source;
/// 3. nothing else: no fuzzy name matching (`firefox-bin` never links to `firefox`).
///
/// SQLite's `lower()` folds ASCII only, which is enough for package names. Takes a plain
/// connection so `replace_source` can call it inside its own transaction.
fn rebuild_links_in(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM app_links", [])?;
    conn.execute(
        "INSERT INTO app_links (source, name, app_key)
         WITH base AS (
             SELECT p.source, p.name,
                    coalesce(
                        p.app_id,
                        (SELECT min(q.app_id) FROM packages q
                          WHERE lower(q.name) = lower(p.name) AND q.app_id IS NOT NULL),
                        lower(p.name)
                    ) AS base
               FROM packages p
         ), ranked AS (
             SELECT source, name, base,
                    row_number() OVER (PARTITION BY source, base ORDER BY name) AS rn
               FROM base
         )
         SELECT source, name, CASE WHEN rn = 1 THEN base ELSE base || '#' || name END
           FROM ranked",
        [],
    )?;
    Ok(())
}

/// FNV-1a over the seed and the name, then a splitmix64 finalizer so short, similar names do not
/// order alike. Deterministic across runs and platforms (unlike `DefaultHasher`).
fn seeded_hash(seed: u64, name: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in seed.to_le_bytes().into_iter().chain(name.bytes()) {
        h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3);
    }
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d049bb133111eb);
    h ^ (h >> 31)
}

fn user_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

/// Rebuilds the schema in one transaction. The version is read again once the write lock is held,
/// so several processes opening an old index at the same time neither fail nor drop each other's
/// freshly created tables: the first migrates, the others see the new version and do nothing.
fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let outcome = (|| -> Result<()> {
        let found = user_version(conn)?;
        if found > SCHEMA_VERSION {
            return Err(Error::UnsupportedIndexVersion {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        if found != SCHEMA_VERSION {
            // Older schema: drop it; the next refresh refills the cache.
            conn.execute_batch(
                "DROP TABLE IF EXISTS packages_fts; DROP TABLE IF EXISTS packages; \
                 DROP TABLE IF EXISTS app_links;",
            )?;
            conn.execute_batch(SCHEMA)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(())
    })();
    match outcome {
        Ok(()) => Ok(conn.execute_batch("COMMIT")?),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

const LOOKUP_INDEXES: [&str; 2] = ["packages_name_lower", "packages_title_lower"];

/// An index written before the lookup indexes existed still works but answers short queries
/// slowly. Creating them is a one-time write, done only when one is missing.
fn ensure_lookup_indexes(conn: &Connection) -> Result<()> {
    let present: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name IN (?1, ?2)",
        params![LOOKUP_INDEXES[0], LOOKUP_INDEXES[1]],
        |r| r.get(0),
    )?;
    if present as usize == LOOKUP_INDEXES.len() {
        return Ok(());
    }
    conn.execute_batch(
        "BEGIN IMMEDIATE;
         CREATE INDEX IF NOT EXISTS packages_name_lower ON packages (lower(name));
         CREATE INDEX IF NOT EXISTS packages_title_lower ON packages (lower(title));
         COMMIT;",
    )?;
    Ok(())
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
        app_id: row.get(10)?,
        categories: categories_from_sql(&row.get::<_, String>(11)?),
        icon: row.get(12)?,
        popularity: row.get(13)?,
        upgradable: row.get(14)?,
    })
}

/// Categories are stored as a JSON array, so a category containing `;` or `,` survives.
fn categories_to_sql(categories: &[String]) -> String {
    serde_json::to_string(categories).unwrap_or_else(|_| "[]".into())
}

fn categories_from_sql(text: &str) -> Vec<String> {
    serde_json::from_str(text).unwrap_or_default()
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
            app_id: None,
            categories: Vec::new(),
            icon: None,
            popularity: None,
            upgradable: false,
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
    fn index_round_trips_new_package_fields() {
        let mut idx = Index::open_in_memory().unwrap();
        let p = Package {
            app_id: Some("org.mozilla.firefox".into()),
            categories: vec!["Network".into(), "WebBrowser".into()],
            icon: Some("/x/y.jxl".into()),
            popularity: Some(1.5),
            upgradable: true,
            ..full("a", "firefox")
        };
        idx.replace_source(&sid("a"), std::slice::from_ref(&p))
            .unwrap();
        assert_eq!(idx.get(&sid("a"), "firefox").unwrap(), Some(p));
    }

    #[test]
    fn categories_with_semicolon_are_not_split_wrongly() {
        let mut idx = Index::open_in_memory().unwrap();
        let p = Package {
            categories: vec!["A;B".into(), "C".into()],
            ..full("a", "x")
        };
        idx.replace_source(&sid("a"), std::slice::from_ref(&p))
            .unwrap();
        let got = idx.get(&sid("a"), "x").unwrap().unwrap();
        assert_eq!(got.categories, vec!["A;B".to_string(), "C".to_string()]);
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
        idx.replace_source(&sid("a"), std::slice::from_ref(&p))
            .unwrap();
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
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn one_letter_query_matches_name_prefixes_only() {
        let idx = indexed(&[
            pkg("a", "banana", "a fruit, as in apple"),
            pkg("a", "apple", "fruit"),
            pkg("a", "alpha", "first letter"),
            pkg("a", "Avocado", "fruit"),
        ]);
        // Names starting with "a" (any case), shorter first, then by name; not the description hit.
        assert_eq!(
            names(idx.search("a", 10).unwrap()),
            ["alpha", "apple", "Avocado"]
        );
    }

    #[test]
    fn two_letter_query_ranks_exact_then_shorter_names() {
        let idx = indexed(&[
            pkg("a", "gopher", "x"),
            pkg("a", "golang", "x"),
            pkg("a", "go", "x"),
            pkg("a", "ago", "x"),
        ]);
        assert_eq!(
            names(idx.search("GO", 10).unwrap()),
            ["go", "golang", "gopher"]
        );
    }

    #[test]
    fn short_query_respects_limit() {
        let many: Vec<Package> = (0..20)
            .map(|i| pkg("a", &format!("tool-{i}"), "x"))
            .collect();
        let idx = indexed(&many);
        assert_eq!(idx.search("t", 5).unwrap().len(), 5);
    }

    #[test]
    fn non_ascii_short_query_still_works() {
        let idx = indexed(&[pkg("a", "éclair", "pastry")]);
        assert_eq!(names(idx.search("é", 10).unwrap()), ["éclair"]);
    }

    #[test]
    fn short_query_uses_the_name_index() {
        let idx = Index::open_in_memory().unwrap();
        let plan: String = idx
            .conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT name FROM packages \
                 WHERE lower(name) >= 'a' AND lower(name) < 'b'",
            )
            .unwrap()
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .map(|r| r.unwrap())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(plan.contains("packages_name_lower"), "{plan}");
    }

    #[test]
    fn exact_title_ranks_with_exact_name() {
        // Many `firefox-*` names start with the query; the app whose title is exactly
        // "Firefox" must still come first, as the plan's headline example requires.
        let mut packages: Vec<Package> = (0..30)
            .map(|i| pkg("a", &format!("firefox-extra-{i}"), "an add-on"))
            .collect();
        packages.push(Package {
            title: Some("Firefox".into()),
            ..pkg("a", "org.mozilla.firefox", "web browser")
        });
        let idx = indexed(&packages);
        assert_eq!(
            idx.search("firefox", 10).unwrap()[0].name,
            "org.mozilla.firefox"
        );
    }

    #[test]
    fn separators_in_the_query_still_count_as_an_exact_name() {
        // bm25 alone would prefer the short competitor: the real package has a long description.
        let long = vec!["word"; 60].join(" ");
        let idx = indexed(&[
            pkg(
                "a",
                "edition-firefox-developer",
                "firefox developer edition",
            ),
            pkg("a", "firefox-developer-edition", &long),
        ]);
        assert_eq!(
            idx.search("firefox developer edition", 10).unwrap()[0].name,
            "firefox-developer-edition"
        );
    }

    #[test]
    fn one_and_two_letter_queries_match_title_prefixes() {
        let idx = indexed(&[
            Package {
                title: Some("Qt Creator".into()),
                ..pkg("a", "org.qt.creator", "an IDE")
            },
            Package {
                title: Some("VLC media player".into()),
                ..pkg("a", "org.videolan.VLC", "player")
            },
            pkg("a", "unrelated", "nothing"),
        ]);
        assert_eq!(names(idx.search("qt", 10).unwrap()), ["org.qt.creator"]);
        assert_eq!(names(idx.search("vl", 10).unwrap()), ["org.videolan.VLC"]);
    }

    fn index_names(path: &std::path::Path) -> Vec<String> {
        let conn = rusqlite::Connection::open(path).unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'packages_%_lower'")
            .unwrap();
        let mut found: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        found.sort();
        found
    }

    #[test]
    fn a_current_index_missing_its_lookup_indexes_gets_them_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        drop(Index::open(&path).unwrap());
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch("DROP INDEX packages_name_lower;")
                .unwrap();
            conn.execute_batch("DROP INDEX IF EXISTS packages_title_lower;")
                .unwrap();
        }
        assert!(index_names(&path).is_empty());
        drop(Index::open(&path).unwrap());
        assert_eq!(
            index_names(&path),
            ["packages_name_lower", "packages_title_lower"]
        );
    }

    #[test]
    fn concurrent_opens_of_an_old_index_all_succeed() {
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
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        }
        let handles: Vec<_> = (0..6)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || Index::open(&path).map(|i| i.count().unwrap()))
            })
            .collect();
        for handle in handles {
            assert_eq!(handle.join().unwrap().expect("every open must succeed"), 0);
        }
        let version: i64 = rusqlite::Connection::open(&path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn short_queries_rank_by_name_length_whether_or_not_there_is_a_title() {
        // A missing title must not sort after a present one (NULL vs 0 in the exact-match key).
        let idx = indexed(&[
            Package {
                title: Some("Qt Creator".into()),
                ..pkg("a", "org.qt.creator.long", "ide")
            },
            pkg("a", "qt5", "no title here"),
        ]);
        assert_eq!(
            names(idx.search("qt", 10).unwrap()),
            ["qt5", "org.qt.creator.long"]
        );
    }

    #[test]
    fn a_source_filter_applies_to_both_search_paths() {
        let mut idx = Index::open_in_memory().unwrap();
        idx.replace_source(
            &SourceId::new("a"),
            &[
                pkg("a", "qtool", "a tool"),
                pkg("a", "toolbox", "a box of tools"),
            ],
        )
        .unwrap();
        idx.replace_source(
            &SourceId::new("b"),
            &[pkg("b", "qtile", "a tiler"), pkg("b", "toolchain", "tools")],
        )
        .unwrap();
        let a = SourceId::new("a");
        assert_eq!(names(idx.search_in("qt", 10, Some(&a)).unwrap()), ["qtool"]);
        assert_eq!(
            names(idx.search_in("tool", 10, Some(&a)).unwrap()),
            ["toolbox", "qtool"]
        );
        assert_eq!(idx.search_in("qt", 10, None).unwrap().len(), 2);
    }

    fn with_app_id(source: &str, name: &str, app_id: &str) -> Package {
        Package {
            app_id: Some(app_id.into()),
            ..pkg(source, name, "x")
        }
    }

    fn multi(packages: &[Package]) -> Index {
        let mut idx = Index::open_in_memory().unwrap();
        for source in ["aur", "flatpak", "pacman"] {
            let of: Vec<Package> = packages
                .iter()
                .filter(|p| p.source.as_str() == source)
                .cloned()
                .collect();
            if !of.is_empty() {
                idx.replace_source(&sid(source), &of).unwrap();
            }
        }
        idx
    }

    fn key(idx: &Index, p: &Package) -> String {
        idx.keys_of(std::slice::from_ref(p)).unwrap().remove(0)
    }

    #[test]
    fn same_app_id_links_pacman_and_flatpak() {
        let a = with_app_id("pacman", "firefox", "org.mozilla.firefox");
        let b = with_app_id("flatpak", "org.mozilla.firefox", "org.mozilla.firefox");
        let idx = multi(&[a.clone(), b.clone()]);
        assert_eq!(key(&idx, &a), "org.mozilla.firefox");
        assert_eq!(key(&idx, &b), "org.mozilla.firefox");
    }

    #[test]
    fn identical_name_links_aur_to_pacman() {
        let a = pkg("pacman", "htop", "x");
        let b = pkg("aur", "htop", "x");
        let idx = multi(&[a.clone(), b.clone()]);
        assert_eq!(key(&idx, &a), "htop");
        assert_eq!(key(&idx, &b), "htop");
    }

    #[test]
    fn name_match_ignores_case() {
        let a = pkg("pacman", "Htop", "x");
        let b = pkg("aur", "htop", "x");
        let idx = multi(&[a.clone(), b.clone()]);
        assert_eq!(key(&idx, &a), key(&idx, &b));
        assert_eq!(key(&idx, &a), "htop");
    }

    #[test]
    fn package_without_app_id_joins_the_app_id_of_a_same_named_package() {
        let a = with_app_id("pacman", "firefox", "org.mozilla.firefox");
        let b = pkg("aur", "firefox", "x");
        let idx = multi(&[a.clone(), b.clone()]);
        assert_eq!(key(&idx, &b), "org.mozilla.firefox");
        assert_eq!(key(&idx, &a), "org.mozilla.firefox");
    }

    #[test]
    fn aur_firefox_bin_does_not_link_to_firefox() {
        let a = pkg("pacman", "firefox", "x");
        let b = pkg("aur", "firefox-bin", "x");
        let idx = multi(&[a.clone(), b.clone()]);
        assert_eq!(key(&idx, &a), "firefox");
        assert_eq!(key(&idx, &b), "firefox-bin");
    }

    #[test]
    fn two_packages_of_one_source_with_same_app_id_stay_separate() {
        let foo = with_app_id("pacman", "foo", "org.x.Foo");
        let extra = with_app_id("pacman", "foo-extra", "org.x.Foo");
        let flat = with_app_id("flatpak", "org.x.Foo", "org.x.Foo");
        let idx = multi(&[foo.clone(), extra.clone(), flat.clone()]);
        assert_eq!(key(&idx, &foo), "org.x.Foo");
        assert_eq!(key(&idx, &flat), "org.x.Foo");
        assert_eq!(key(&idx, &extra), "org.x.Foo#foo-extra");
    }

    #[test]
    fn links_are_rebuilt_when_another_source_arrives() {
        let mut idx = Index::open_in_memory().unwrap();
        let aur = pkg("aur", "firefox", "x");
        idx.replace_source(&sid("aur"), std::slice::from_ref(&aur))
            .unwrap();
        assert_eq!(key(&idx, &aur), "firefox");
        idx.replace_source(
            &sid("pacman"),
            &[with_app_id("pacman", "firefox", "org.mozilla.firefox")],
        )
        .unwrap();
        assert_eq!(key(&idx, &aur), "org.mozilla.firefox");
    }

    #[test]
    fn replacing_a_source_removes_its_old_links() {
        let mut idx = Index::open_in_memory().unwrap();
        idx.replace_source(&sid("aur"), &[pkg("aur", "old", "x")])
            .unwrap();
        idx.replace_source(&sid("aur"), &[pkg("aur", "new", "x")])
            .unwrap();
        let n: i64 = idx
            .conn
            .query_row("SELECT COUNT(*) FROM app_links", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        assert!(idx.members("old").unwrap().is_empty());
        assert_eq!(names(idx.members("new").unwrap()), ["new"]);
    }

    #[test]
    fn rebuild_is_idempotent() {
        let mut idx = multi(&[
            with_app_id("pacman", "foo", "org.x.Foo"),
            with_app_id("pacman", "foo-extra", "org.x.Foo"),
            pkg("aur", "foo", "x"),
        ]);
        let dump = |idx: &Index| -> Vec<(String, String, String)> {
            idx.conn
                .prepare("SELECT source, name, app_key FROM app_links ORDER BY source, name")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        let before = dump(&idx);
        assert_eq!(before.len(), 3);
        idx.rebuild_links().unwrap();
        idx.rebuild_links().unwrap();
        assert_eq!(dump(&idx), before);
    }

    #[test]
    fn keys_of_keeps_the_input_order() {
        let idx = multi(&[
            pkg("pacman", "b", "x"),
            pkg("pacman", "a", "x"),
            pkg("aur", "c", "x"),
        ]);
        let ask = [
            pkg("aur", "c", "x"),
            pkg("pacman", "a", "x"),
            pkg("pacman", "b", "x"),
        ];
        assert_eq!(idx.keys_of(&ask).unwrap(), ["c", "a", "b"]);
        assert!(idx.keys_of(&[pkg("aur", "missing", "x")]).is_err());
    }

    #[test]
    fn members_returns_every_source() {
        let idx = multi(&[
            pkg("pacman", "htop", "x"),
            pkg("aur", "htop", "x"),
            pkg("pacman", "other", "x"),
        ]);
        let got: Vec<(String, String)> = idx
            .members("htop")
            .unwrap()
            .into_iter()
            .map(|p| (p.source.to_string(), p.name))
            .collect();
        assert_eq!(
            got,
            [
                ("aur".into(), "htop".into()),
                ("pacman".into(), "htop".into())
            ]
        );
        assert!(idx.members("nope").unwrap().is_empty());
    }

    #[test]
    fn source_counts_counts_each_source() {
        let idx = multi(&[
            pkg("pacman", "a", "x"),
            pkg("pacman", "b", "x"),
            pkg("aur", "c", "x"),
        ]);
        assert_eq!(
            idx.source_counts().unwrap(),
            [(sid("aur"), 1), (sid("pacman"), 2)]
        );
    }

    fn popular(name: &str, popularity: Option<f64>) -> Package {
        Package {
            popularity,
            ..pkg("aur", name, "x")
        }
    }

    #[test]
    fn featured_aur_orders_by_popularity() {
        let idx = multi(&[
            popular("low", Some(1.0)),
            popular("none", None),
            popular("high", Some(9.0)),
            popular("mid", Some(5.0)),
        ]);
        let got = names(idx.featured(&sid("aur"), 3, 7).unwrap());
        assert_eq!(got, ["high", "mid", "low"]);
        let all = names(idx.featured(&sid("aur"), 10, 7).unwrap());
        assert_eq!(all.last().unwrap(), "none");
    }

    fn with_icon(source: &str, name: &str) -> Package {
        Package {
            icon: Some(format!("/icons/{name}.png")),
            ..pkg(source, name, "x")
        }
    }

    #[test]
    fn featured_prefers_packages_with_icons() {
        let idx = multi(&[
            pkg("pacman", "a", "x"),
            with_icon("pacman", "b"),
            pkg("pacman", "c", "x"),
            with_icon("pacman", "d"),
        ]);
        let mut got = names(idx.featured(&sid("pacman"), 10, 1).unwrap());
        got.sort();
        assert_eq!(got, ["b", "d"]);
    }

    #[test]
    fn featured_is_stable_for_a_seed_and_varies_between_seeds() {
        let many: Vec<Package> = (0..50)
            .map(|i| with_icon("pacman", &format!("app{i:02}")))
            .collect();
        let idx = multi(&many);
        let pacman = sid("pacman");
        let first = names(idx.featured(&pacman, 10, 42).unwrap());
        assert_eq!(first.len(), 10);
        assert_eq!(first, names(idx.featured(&pacman, 10, 42).unwrap()));
        let others: Vec<Vec<String>> = (0..5)
            .map(|s| names(idx.featured(&pacman, 10, s).unwrap()))
            .collect();
        assert!(others.iter().any(|o| *o != first));
        assert!(others.iter().any(|o| *o != others[0]));
    }

    #[test]
    fn featured_falls_back_when_no_icons() {
        let idx = multi(&[
            pkg("pacman", "c", "x"),
            pkg("pacman", "a", "x"),
            pkg("pacman", "b", "x"),
        ]);
        assert_eq!(
            names(idx.featured(&sid("pacman"), 2, 3).unwrap()),
            ["a", "b"]
        );
    }

    #[test]
    fn upgradable_lists_only_flagged_packages() {
        let up = |source: &str, name: &str| Package {
            upgradable: true,
            ..pkg(source, name, "x")
        };
        let idx = multi(&[
            up("pacman", "zed"),
            pkg("pacman", "calm", "x"),
            up("aur", "zed"),
            up("pacman", "abc"),
        ]);
        let got: Vec<(String, String)> = idx
            .upgradable()
            .unwrap()
            .into_iter()
            .map(|p| (p.name, p.source.to_string()))
            .collect();
        assert_eq!(
            got,
            [
                ("abc".into(), "pacman".into()),
                ("zed".into(), "aur".into()),
                ("zed".into(), "pacman".into())
            ]
        );
    }

    #[test]
    fn v3_index_gains_the_links_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE packages (id INTEGER PRIMARY KEY, source TEXT NOT NULL,
                     name TEXT NOT NULL, UNIQUE (source, name));
                 PRAGMA user_version = 3;",
            )
            .unwrap();
        }
        let mut idx = Index::open(&path).unwrap();
        idx.replace_source(&sid("a"), &[pkg("a", "x", "d")])
            .unwrap();
        assert_eq!(idx.keys_of(&[pkg("a", "x", "d")]).unwrap(), ["x"]);
    }

    /// 150k synthetic packages in three sources; prints the cost of the link rebuild.
    #[test]
    #[ignore]
    fn rebuild_links_timing() {
        let mut idx = Index::open_in_memory().unwrap();
        let make = |source: &str, n: usize, with_ids: bool| -> Vec<Package> {
            (0..n)
                .map(|i| Package {
                    // Names repeat across sources, so links actually merge.
                    app_id: (with_ids && i % 3 == 0).then(|| format!("org.example.App{i}")),
                    ..pkg(source, &format!("pkg-{i}"), "synthetic")
                })
                .collect()
        };
        let started = std::time::Instant::now();
        idx.replace_source(&sid("pacman"), &make("pacman", 15_000, true))
            .unwrap();
        idx.replace_source(&sid("flatpak"), &make("flatpak", 5_000, true))
            .unwrap();
        let aur = make("aur", 130_000, false);
        let t = std::time::Instant::now();
        idx.replace_source(&sid("aur"), &aur).unwrap();
        let replace = t.elapsed();
        let t = std::time::Instant::now();
        idx.rebuild_links().unwrap();
        let rebuild = t.elapsed();
        println!(
            "count {} | replace_source(aur, 130k, includes link rebuild) {replace:?} | rebuild_links {rebuild:?} | all {:?}",
            idx.count().unwrap(),
            started.elapsed()
        );
    }
}
