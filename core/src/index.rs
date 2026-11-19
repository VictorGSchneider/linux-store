use std::path::Path;

use rusqlite::{Connection, Row, params};

use crate::{Error, Package, Result, SourceId};

const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS packages (
        id INTEGER PRIMARY KEY,
        source TEXT NOT NULL,
        name TEXT NOT NULL,
        version TEXT NOT NULL,
        installed_version TEXT,
        description TEXT,
        repo TEXT,
        url TEXT,
        download_size INTEGER NOT NULL,
        installed_size INTEGER NOT NULL,
        UNIQUE (source, name)
    );
    CREATE VIRTUAL TABLE IF NOT EXISTS packages_fts
        USING fts5(name, description, tokenize = 'unicode61');
";

/// Columns of `packages`, in the order `row_to_package` reads them.
const COLUMNS: &str = "source, name, version, installed_version, description, repo, url, \
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

    fn init(conn: Connection) -> Result<Index> {
        conn.execute_batch(SCHEMA)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Index { conn })
    }

    /// Atomically replaces everything stored for `source` with `packages`.
    pub fn replace_source(&mut self, source: &SourceId, packages: &[Package]) -> Result<()> {
        if let Some(bad) = packages.iter().find(|p| &p.source != source) {
            return Err(Error::SourceMismatch { expected: source.clone(), found: bad.source.clone() });
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM packages_fts WHERE rowid IN (SELECT id FROM packages WHERE source = ?1)",
            [source.as_str()],
        )?;
        tx.execute("DELETE FROM packages WHERE source = ?1", [source.as_str()])?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO packages (source, name, version, installed_version, description, \
                 repo, url, download_size, installed_size) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            let mut insert_fts =
                tx.prepare("INSERT INTO packages_fts (rowid, name, description) VALUES (?1, ?2, ?3)")?;
            for p in packages {
                insert.execute(params![
                    p.source.as_str(),
                    p.name,
                    p.version,
                    p.installed_version,
                    p.description,
                    p.repo,
                    p.url,
                    to_sql_size(p.download_size),
                    to_sql_size(p.installed_size),
                ])?;
                let id = tx.last_insert_rowid();
                insert_fts.execute(params![id, p.name, p.description.as_deref().unwrap_or("")])?;
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

    pub fn count(&self) -> Result<usize> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM packages", [], |r| r.get(0))?;
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
        description: row.get(4)?,
        repo: row.get(5)?,
        url: row.get(6)?,
        download_size: row.get::<_, i64>(7)?.max(0) as u64,
        installed_size: row.get::<_, i64>(8)?.max(0) as u64,
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
            description: Some("A tool".into()),
            repo: Some("extra".into()),
            url: Some("https://example.org".into()),
            download_size: 1234,
            installed_size: 5678,
        }
    }

    fn installed(source: &str, name: &str) -> Package {
        Package { installed_version: Some("1.0".into()), ..pkg(source, name, "x") }
    }

    #[test]
    fn roundtrip_preserves_every_field() {
        let mut idx = Index::open_in_memory().unwrap();
        let bare = Package {
            description: None,
            repo: None,
            url: None,
            installed_version: None,
            ..full("a", "bare")
        };
        let rich = full("a", "rich");
        idx.replace_source(&sid("a"), &[rich.clone(), bare.clone()]).unwrap();
        assert_eq!(idx.get(&sid("a"), "rich").unwrap(), Some(rich));
        assert_eq!(idx.get(&sid("a"), "bare").unwrap(), Some(bare));
        assert_eq!(idx.get(&sid("a"), "missing").unwrap(), None);
    }

    #[test]
    fn replace_source_replaces_only_that_source() {
        let mut idx = Index::open_in_memory().unwrap();
        idx.replace_source(&sid("a"), &[pkg("a", "x", "d")]).unwrap();
        idx.replace_source(&sid("b"), &[pkg("b", "y", "d")]).unwrap();
        idx.replace_source(&sid("a"), &[pkg("a", "z", "d")]).unwrap();
        assert_eq!(idx.get(&sid("a"), "x").unwrap(), None);
        assert!(idx.get(&sid("a"), "z").unwrap().is_some());
        assert!(idx.get(&sid("b"), "y").unwrap().is_some());
        assert_eq!(idx.count().unwrap(), 2);
    }

    #[test]
    fn replace_source_is_atomic_on_mismatch() {
        let mut idx = Index::open_in_memory().unwrap();
        idx.replace_source(&sid("a"), &[pkg("a", "x", "d")]).unwrap();
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
        idx.replace_source(&sid("a"), &[pkg("a", "x", "d")]).unwrap();
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
            &[installed("a", "zsh"), pkg("a", "git", "d"), installed("a", "bash")],
        )
        .unwrap();
        let names: Vec<String> = idx.installed().unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["bash", "zsh"]);
    }

    #[test]
    fn index_persists_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let mut idx = Index::open(&path).unwrap();
            idx.replace_source(&sid("a"), &[pkg("a", "x", "d"), pkg("a", "y", "d")]).unwrap();
        }
        assert_eq!(Index::open(&path).unwrap().count().unwrap(), 2);
    }
}
