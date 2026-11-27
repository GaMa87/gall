//! SQLite store: per-folder preferences (V-02, F-02, F-03), folder history
//! (S-04), colour tags (O-14) and recent destinations (N-11).

use crate::error::Result;
use crate::view::sort::{GroupMode, GroupOrder, SortKey};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

pub struct Db {
    conn: Mutex<Connection>,
}

/// Explicit per-folder settings; `None` = not set for this folder.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "StoredPrefs")]
pub struct FolderPrefs {
    pub sort_key: Option<SortKey>,
    pub descending: Option<bool>,
    pub group: Option<GroupMode>,
    pub group_order: Option<GroupOrder>,
    pub view: Option<String>,
    /// The collapsed groups of this folder.
    pub collapsed: Option<Vec<String>>,
}

/// As a session of an older Gall wrote them: with `foldersOnTop` instead of `group`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredPrefs {
    sort_key: Option<SortKey>,
    descending: Option<bool>,
    folders_on_top: Option<bool>,
    group: Option<GroupMode>,
    group_order: Option<GroupOrder>,
    view: Option<String>,
    collapsed: Option<Vec<String>>,
}

impl From<StoredPrefs> for FolderPrefs {
    fn from(p: StoredPrefs) -> Self {
        FolderPrefs {
            sort_key: p.sort_key,
            descending: p.descending,
            group: p
                .group
                .or_else(|| p.folders_on_top.map(GroupMode::from_folders_on_top)),
            group_order: p.group_order,
            view: p.view,
            collapsed: p.collapsed,
        }
    }
}

/// Effective preferences after inheritance and defaults.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPrefs {
    /// The folder these are for.
    pub path: String,
    pub sort_key: SortKey,
    pub descending: bool,
    pub group: GroupMode,
    pub group_order: GroupOrder,
    pub view: String,
    /// Folder the view was inherited from (None = default).
    pub view_source: Option<String>,
    /// Folder the grouping was inherited from (None = default).
    pub group_source: Option<String>,
    pub collapsed: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tagged {
    pub path: String,
    pub color: u8,
}

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS folder_prefs (
    path TEXT PRIMARY KEY,
    sort_key TEXT,
    descending INTEGER,
    folders_on_top INTEGER,
    view TEXT
);
CREATE TABLE IF NOT EXISTS history (path TEXT PRIMARY KEY, visited INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS history_visited ON history(visited DESC);
CREATE TABLE IF NOT EXISTS tags (path TEXT PRIMARY KEY, color INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS destinations (path TEXT PRIMARY KEY, used INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS pair_gravity (
    path_a TEXT NOT NULL,
    path_b TEXT NOT NULL,
    score REAL NOT NULL,
    last_used INTEGER NOT NULL,
    suppressed INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (path_a, path_b)
);
CREATE INDEX IF NOT EXISTS pair_gravity_a ON pair_gravity(path_a, suppressed, score DESC);
CREATE INDEX IF NOT EXISTS pair_gravity_b ON pair_gravity(path_b, suppressed, score DESC);
"#;

/// The smallest text greater than every text that begins with `prefix` (which ends with
/// a path separator): paths are compared byte by byte, so `path >= prefix AND path <
/// after_prefix(prefix)` is "begins with `prefix`".
fn after_prefix(prefix: &str) -> String {
    let mut after = prefix.to_owned();
    let last = after.pop().expect("a prefix ends with a separator");
    debug_assert!(last.is_ascii());
    after.push((last as u8 + 1) as char);
    after
}

fn key_to_str(k: SortKey) -> &'static str {
    match k {
        SortKey::Name => "name",
        SortKey::Ext => "ext",
        SortKey::Modified => "modified",
        SortKey::Created => "created",
        SortKey::Size => "size",
    }
}

fn key_from_str(s: &str) -> Option<SortKey> {
    Some(match s {
        "name" => SortKey::Name,
        "ext" => SortKey::Ext,
        "modified" => SortKey::Modified,
        "created" => SortKey::Created,
        "size" => SortKey::Size,
        _ => return None,
    })
}

/// Adds the columns for grouping to the preferences of an older Gall. What "folders on top" was
/// in a folder becomes its grouping, so every list keeps its order.
fn add_grouping(conn: &Connection) -> Result<()> {
    let missing = |conn: &Connection| -> Result<bool> {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('folder_prefs') WHERE name = 'grouping'",
            [],
            |r| r.get::<_, i64>(0),
        )? == 0)
    };
    if !missing(conn)? {
        return Ok(());
    }
    // Alone: a second instance may be starting now.
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let done = missing(conn).and_then(|missing| {
        if missing {
            conn.execute_batch(
                "ALTER TABLE folder_prefs ADD COLUMN grouping TEXT;
                 ALTER TABLE folder_prefs ADD COLUMN group_order TEXT;
                 ALTER TABLE folder_prefs ADD COLUMN collapsed TEXT;
                 UPDATE folder_prefs
                    SET grouping = CASE folders_on_top WHEN 0 THEN 'none' ELSE 'folders' END
                  WHERE folders_on_top IS NOT NULL;",
            )?;
        }
        Ok(())
    });
    conn.execute_batch(if done.is_ok() { "COMMIT" } else { "ROLLBACK" })?;
    done
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        conn.execute_batch(SCHEMA)?;
        add_grouping(&conn)?;
        // With the write-ahead log a commit need not wait for the disk: after a power
        // cut the last changes may be missing, the database is never damaged. And a
        // second instance writing at the same moment is waited for, not an error.
        // Cap the WAL well below SQLite's 1000-page (~4 MB) default so it never
        // sits bloated beside a ~50 KB database, and truncate it after checkpoints.
        const WAL_SIZE_LIMIT: i64 = 262_144;
        conn.execute_batch(&format!(
            "PRAGMA synchronous = NORMAL;
             PRAGMA wal_autocheckpoint = 64;
             PRAGMA journal_size_limit = {WAL_SIZE_LIMIT};"
        ))?;
        let wal_path = {
            let mut s = path.as_os_str().to_os_string();
            s.push("-wal");
            std::path::PathBuf::from(s)
        };
        if std::fs::metadata(&wal_path)
            .map(|m| m.len() > WAL_SIZE_LIMIT as u64)
            .unwrap_or(false)
        {
            // Shrink an oversized WAL left by an older version or unclean exit without
            // stalling startup if another instance holds a read lock.
            let _ = conn.busy_timeout(std::time::Duration::from_millis(50));
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            let _ = conn.busy_timeout(std::time::Duration::from_secs(2));
        }
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    /// Flushes and truncates the write-ahead log on clean shutdown so `-wal`
    /// does not linger on disk between runs.
    pub fn checkpoint(&self) {
        if let Ok(c) = self.conn.lock() {
            let _ = c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        }
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        add_grouping(&conn)?;
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    pub fn get_prefs(&self, path: &str) -> Result<FolderPrefs> {
        let c = self.conn.lock().unwrap();
        let r = c
            .query_row(
                "SELECT sort_key, descending, grouping, group_order, view, collapsed FROM folder_prefs WHERE path = ?1",
                params![path],
                |r| {
                    Ok(FolderPrefs {
                        sort_key: r.get::<_, Option<String>>(0)?.as_deref().and_then(key_from_str),
                        descending: r.get(1)?,
                        group: r.get::<_, Option<String>>(2)?.as_deref().and_then(GroupMode::parse),
                        group_order: r.get::<_, Option<String>>(3)?.as_deref().and_then(GroupOrder::parse),
                        view: r.get(4)?,
                        collapsed: r
                            .get::<_, Option<String>>(5)?
                            .and_then(|text| serde_json::from_str(&text).ok()),
                    })
                },
            )
            .optional()?;
        Ok(r.unwrap_or_default())
    }

    /// Merges the given fields into the stored row (None fields are untouched).
    pub fn set_prefs(&self, path: &str, p: &FolderPrefs) -> Result<()> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO folder_prefs (path, sort_key, descending, grouping, group_order, view, collapsed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(path) DO UPDATE SET
               sort_key = COALESCE(excluded.sort_key, sort_key),
               descending = COALESCE(excluded.descending, descending),
               grouping = COALESCE(excluded.grouping, grouping),
               group_order = COALESCE(excluded.group_order, group_order),
               view = COALESCE(excluded.view, view),
               collapsed = COALESCE(excluded.collapsed, collapsed)",
            params![
                path,
                p.sort_key.map(key_to_str),
                p.descending,
                p.group.map(GroupMode::as_str),
                p.group_order.map(GroupOrder::as_str),
                p.view,
                p.collapsed
                    .as_ref()
                    .and_then(|ids| serde_json::to_string(ids).ok()),
            ],
        )?;
        Ok(())
    }

    /// Forgets the sort and view kept for every folder.
    pub fn clear_prefs(&self) -> Result<()> {
        let c = self.conn.lock().unwrap();
        c.execute("DELETE FROM folder_prefs", [])?;
        Ok(())
    }

    /// Resolves effective prefs. Sort is per-folder (`M:folderpane`); view and
    /// grouping (before: folders on top) inherit from the nearest ancestor that sets them (`M:fileviews`).
    pub fn resolve_prefs(
        &self,
        path: &Path,
        default_sort: SortKey,
        default_desc: bool,
        default_group: GroupMode,
        default_order: GroupOrder,
        default_view: &str,
    ) -> Result<ResolvedPrefs> {
        let own = self.get_prefs(&path.to_string_lossy())?;
        let mut res = ResolvedPrefs {
            sort_key: own.sort_key.unwrap_or(default_sort),
            descending: own.descending.unwrap_or(default_desc),
            path: path.to_string_lossy().into_owned(),
            group: default_group,
            group_order: default_order,
            view: default_view.to_string(),
            view_source: None,
            group_source: None,
            collapsed: own.collapsed.clone().unwrap_or_default(),
        };
        let (mut got_view, mut got_group, mut got_order) = (false, false, false);
        for anc in path.ancestors() {
            if anc.as_os_str().is_empty() {
                continue;
            }
            let key = anc.to_string_lossy();
            let p = if anc == path {
                own.clone()
            } else {
                self.get_prefs(&key)?
            };
            if !got_view {
                if let Some(v) = p.view {
                    res.view = v;
                    res.view_source = Some(key.to_string());
                    got_view = true;
                }
            }
            if !got_group {
                if let Some(group) = p.group {
                    res.group = group;
                    res.group_source = Some(key.to_string());
                    got_group = true;
                }
            }
            if !got_order {
                if let Some(order) = p.group_order {
                    res.group_order = order;
                    got_order = true;
                }
            }
            if got_view && got_group && got_order {
                break;
            }
        }
        Ok(res)
    }

    pub fn record_visit(&self, path: &str, keep: u32) -> Result<()> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute(
            "INSERT INTO history(path, visited) VALUES(?1, ?2) ON CONFLICT(path) DO UPDATE SET visited = excluded.visited",
            params![path, now()],
        )?;
        tx.execute(
            "DELETE FROM history WHERE path NOT IN (SELECT path FROM history ORDER BY visited DESC LIMIT ?1)",
            params![keep.max(1)],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn history(&self, limit: u32) -> Result<Vec<String>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare("SELECT path FROM history ORDER BY visited DESC LIMIT ?1")?;
        let v = st
            .query_map(params![limit], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(v)
    }

    pub fn remove_history(&self, path: Option<&str>) -> Result<()> {
        let c = self.conn.lock().unwrap();
        match path {
            Some(p) => c.execute("DELETE FROM history WHERE path = ?1", params![p])?,
            None => c.execute("DELETE FROM history", [])?,
        };
        Ok(())
    }

    /// `color = 0` removes the tag.
    pub fn set_tag(&self, path: &str, color: u8) -> Result<()> {
        let c = self.conn.lock().unwrap();
        if color == 0 {
            c.execute("DELETE FROM tags WHERE path = ?1", params![path])?;
        } else {
            c.execute(
                "INSERT INTO tags(path, color) VALUES(?1, ?2) ON CONFLICT(path) DO UPDATE SET color = excluded.color",
                params![path, color],
            )?;
        }
        Ok(())
    }

    /// `set_tag` for many paths, all or none: one transaction, so one write to disk
    /// instead of one per path.
    pub fn set_tags(&self, paths: &[String], color: u8) -> Result<()> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        {
            let mut st = tx.prepare(if color == 0 {
                "DELETE FROM tags WHERE path = ?1"
            } else {
                "INSERT INTO tags(path, color) VALUES(?1, ?2) ON CONFLICT(path) DO UPDATE SET color = excluded.color"
            })?;
            for path in paths {
                if color == 0 {
                    st.execute(params![path])?;
                } else {
                    st.execute(params![path, color])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Tags of direct children of `dir` (for the file list).
    pub fn tags_in(&self, dir: &str) -> Result<Vec<Tagged>> {
        let sep = std::path::MAIN_SEPARATOR;
        let prefix = if dir.ends_with(sep) {
            dir.to_string()
        } else {
            format!("{dir}{sep}")
        };
        let c = self.conn.lock().unwrap();
        // Everything below the folder is one range of the primary key: found through
        // the index instead of reading every tag there is.
        let mut st =
            c.prepare_cached("SELECT path, color FROM tags WHERE path >= ?1 AND path < ?2")?;
        let rows = st.query_map(params![prefix, after_prefix(&prefix)], |r| {
            Ok(Tagged {
                path: r.get(0)?,
                color: r.get(1)?,
            })
        })?;
        let mut out = vec![];
        for t in rows.flatten() {
            if !t.path[prefix.len()..].contains(sep) {
                out.push(t);
            }
        }
        Ok(out)
    }

    pub fn tagged(&self, color: Option<u8>) -> Result<Vec<Tagged>> {
        let c = self.conn.lock().unwrap();
        let mut st =
            c.prepare("SELECT path, color FROM tags WHERE ?1 IS NULL OR color = ?1 ORDER BY path")?;
        let v = st
            .query_map(params![color], |r| {
                Ok(Tagged {
                    path: r.get(0)?,
                    color: r.get(1)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(v)
    }

    /// Keeps tags attached when items are renamed/moved inside Gall.
    /// An item was moved or renamed: its colour tag and folder view settings follow it,
    /// and so do those of everything inside it (a renamed folder).
    pub fn move_tag(&self, from: &str, to: &str) -> Result<()> {
        let c = self.conn.lock().unwrap();
        // The item itself and, as one range of the primary key, everything inside it.
        let inside = format!("{from}{}", std::path::MAIN_SEPARATOR);
        for table in ["tags", "folder_prefs"] {
            c.prepare_cached(&format!(
                "UPDATE OR REPLACE {table} SET path = ?2 || substr(path, length(?1) + 1) \
                 WHERE path = ?1 OR (path >= ?3 AND path < ?4)"
            ))?
            .execute(params![from, to, inside, after_prefix(&inside)])?;
        }
        Ok(())
    }

    pub fn record_destination(&self, path: &str, keep: u32) -> Result<()> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute(
            "INSERT INTO destinations(path, used) VALUES(?1, ?2) ON CONFLICT(path) DO UPDATE SET used = excluded.used",
            params![path, now()],
        )?;
        tx.execute(
            "DELETE FROM destinations WHERE path NOT IN (SELECT path FROM destinations ORDER BY used DESC LIMIT ?1)",
            params![keep.max(1)],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn destinations(&self) -> Result<Vec<String>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare("SELECT path FROM destinations ORDER BY used DESC")?;
        let v = st
            .query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(v)
    }

    pub fn clear_destinations(&self) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM destinations", [])?;
        Ok(())
    }

    // ───────────── pair gravity (WN-181) ─────────────

    /// Increments learned affinity between two folder paths with exponential recency decay.
    /// When `unsuppress` is true (explicit manual pairing), clears any earlier dismissal.
    pub fn record_gravity(&self, a: &str, b: &str, weight: f64, unsuppress: bool) -> Result<()> {
        self.record_gravity_at(a, b, weight, unsuppress, now())
    }

    /// Timestamp-explicit variant for deterministic decay tests.
    pub fn record_gravity_at(
        &self,
        a: &str,
        b: &str,
        weight: f64,
        unsuppress: bool,
        at_ms: i64,
    ) -> Result<()> {
        use super::gravity::{canonical_pair, decayed_score, GRAVITY_MAX_SCORE};

        let Some((pa, pb)) = canonical_pair(a, b) else {
            return Ok(());
        };
        let c = self.conn.lock().unwrap();
        let existing: Option<(f64, i64, i64)> = c
            .query_row(
                "SELECT score, last_used, suppressed FROM pair_gravity WHERE path_a = ?1 AND path_b = ?2",
                params![pa, pb],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (next_score, suppressed) = match existing {
            Some((old_score, last_used, sup)) => {
                let decayed = decayed_score(old_score, last_used, at_ms);
                let next = (decayed + weight).min(GRAVITY_MAX_SCORE);
                let next_sup = if unsuppress { 0 } else { sup };
                (next, next_sup)
            }
            None => (weight.min(GRAVITY_MAX_SCORE), 0),
        };
        c.execute(
            "INSERT INTO pair_gravity (path_a, path_b, score, last_used, suppressed)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path_a, path_b) DO UPDATE SET
               score = excluded.score,
               last_used = excluded.last_used,
               suppressed = excluded.suppressed",
            params![pa, pb, next_score, at_ms, suppressed],
        )?;
        Ok(())
    }

    /// Suppresses future ghost-tab suggestions between `a` and `b`.
    pub fn suppress_gravity(&self, a: &str, b: &str) -> Result<()> {
        use super::gravity::canonical_pair;

        let Some((pa, pb)) = canonical_pair(a, b) else {
            return Ok(());
        };
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO pair_gravity (path_a, path_b, score, last_used, suppressed)
             VALUES (?1, ?2, 0.0, ?3, 1)
             ON CONFLICT(path_a, path_b) DO UPDATE SET
               suppressed = 1,
               last_used = excluded.last_used",
            params![pa, pb, now()],
        )?;
        Ok(())
    }

    pub fn is_gravity_suppressed(&self, a: &str, b: &str) -> Result<bool> {
        use super::gravity::canonical_pair;

        let Some((pa, pb)) = canonical_pair(a, b) else {
            return Ok(false);
        };
        let c = self.conn.lock().unwrap();
        let sup: Option<i64> = c
            .query_row(
                "SELECT suppressed FROM pair_gravity WHERE path_a = ?1 AND path_b = ?2",
                params![pa, pb],
                |r| r.get(0),
            )
            .optional()?;
        Ok(sup == Some(1))
    }

    /// Returns non-suppressed learned partner paths whose decayed score meets `GRAVITY_THRESHOLD`,
    /// ordered highest score first.
    pub fn gravity_candidates(
        &self,
        path: &str,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<(String, f64)>> {
        use super::gravity::{decayed_score, normalize_gravity_path, GRAVITY_THRESHOLD};

        let norm = normalize_gravity_path(path);
        if norm.is_empty() {
            return Ok(Vec::new());
        }
        let c = self.conn.lock().unwrap();
        let fetch_limit = (limit.max(1) * 4) as i64;
        let mut st = c.prepare(
            "SELECT path_b, score, last_used FROM pair_gravity
               WHERE path_a = ?1 AND suppressed = 0
             UNION ALL
             SELECT path_a, score, last_used FROM pair_gravity
               WHERE path_b = ?1 AND suppressed = 0
             ORDER BY score DESC
             LIMIT ?2",
        )?;
        let mut hits: Vec<(String, f64)> = st
            .query_map(params![norm, fetch_limit], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?
            .filter_map(|row| row.ok())
            .filter_map(|(partner, score, last_used)| {
                let d = decayed_score(score, last_used, now_ms);
                (d >= GRAVITY_THRESHOLD).then_some((partner, d))
            })
            .collect();
        hits.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        hits.truncate(limit);
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn inheritance() {
        let db = Db::open_in_memory().unwrap();
        db.set_prefs(
            "/",
            &FolderPrefs {
                view: Some("thumbs".into()),
                group: Some(GroupMode::None),
                ..Default::default()
            },
        )
        .unwrap();
        db.set_prefs(
            "/a",
            &FolderPrefs {
                sort_key: Some(SortKey::Size),
                ..Default::default()
            },
        )
        .unwrap();
        db.set_prefs(
            "/a/b",
            &FolderPrefs {
                view: Some("names".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let r = db
            .resolve_prefs(
                Path::new("/a/b/c"),
                SortKey::Name,
                false,
                GroupMode::Folders,
                GroupOrder::Name,
                "default",
            )
            .unwrap();
        assert_eq!(r.view, "names");
        assert_eq!(r.view_source.as_deref(), Some("/a/b"));
        assert_eq!(r.group, GroupMode::None, "inherited from root");
        assert_eq!(r.group_source.as_deref(), Some("/"));
        assert_eq!(r.sort_key, SortKey::Name, "sort is not inherited");

        let r = db
            .resolve_prefs(
                Path::new("/a"),
                SortKey::Name,
                false,
                GroupMode::Folders,
                GroupOrder::Name,
                "default",
            )
            .unwrap();
        assert_eq!(r.sort_key, SortKey::Size);
        assert_eq!(r.view, "thumbs");

        // Merge semantics: setting one field keeps the others.
        db.set_prefs(
            "/a",
            &FolderPrefs {
                descending: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(db.get_prefs("/a").unwrap().sort_key, Some(SortKey::Size));
    }

    #[test]
    fn folders_on_top_of_an_older_database_becomes_the_grouping() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("gall.db");
        {
            let old = Connection::open(&file).unwrap();
            old.execute_batch(SCHEMA).unwrap();
            old.execute_batch(
                "INSERT INTO folder_prefs (path, folders_on_top) VALUES ('on', 1), ('off', 0);
                 INSERT INTO folder_prefs (path, sort_key, descending, view) VALUES ('sort', 'size', 1, 'grid');",
            )
            .unwrap();
        }
        // The second time finds the columns and changes nothing.
        for _ in 0..2 {
            let db = Db::open(&file).unwrap();
            let group = |path: &str| db.get_prefs(path).unwrap().group;
            assert_eq!(group("on"), Some(GroupMode::Folders));
            assert_eq!(group("off"), Some(GroupMode::None));
            assert_eq!(group("sort"), None);
            let sort = db.get_prefs("sort").unwrap();
            assert_eq!(sort.sort_key, Some(SortKey::Size));
            assert_eq!(sort.view.as_deref(), Some("grid"));
        }
        // A choice made since then is not replaced by the old column.
        let db = Db::open(&file).unwrap();
        let choice = FolderPrefs {
            group: Some(GroupMode::Type),
            group_order: Some(GroupOrder::Recent),
            collapsed: Some(vec!["ext:png".into()]),
            ..Default::default()
        };
        db.set_prefs("on", &choice).unwrap();
        drop(db);
        let db = Db::open(&file).unwrap();
        assert_eq!(db.get_prefs("on").unwrap(), choice);
        // Nothing collapsed is a value too.
        db.set_prefs(
            "on",
            &FolderPrefs {
                collapsed: Some(Vec::new()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(db.get_prefs("on").unwrap().collapsed, Some(Vec::new()));
    }

    #[test]
    fn a_session_of_an_older_gall_keeps_folders_on_top() {
        let p: FolderPrefs = serde_json::from_str(r#"{"foldersOnTop":false}"#).unwrap();
        assert_eq!(p.group, Some(GroupMode::None));
        let p: FolderPrefs =
            serde_json::from_str(r#"{"foldersOnTop":true,"group":"type"}"#).unwrap();
        assert_eq!(p.group, Some(GroupMode::Type));
        let p: FolderPrefs = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(p.group, Some(GroupMode::Type));
    }

    #[test]
    fn history_dedup_and_cap() {
        let db = Db::open_in_memory().unwrap();
        for p in ["/a", "/b", "/c", "/a"] {
            db.record_visit(p, 3).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(db.history(10).unwrap(), ["/a", "/c", "/b"]);
        db.record_visit("/d", 3).unwrap();
        assert_eq!(db.history(10).unwrap().len(), 3);
    }

    #[cfg(unix)]
    #[test]
    fn tags() {
        let db = Db::open_in_memory().unwrap();
        db.set_tag("/d/a.txt", 2).unwrap();
        db.set_tag("/d/sub/b.txt", 3).unwrap();
        let t = db.tags_in("/d").unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].color, 2);
        db.move_tag("/d/a.txt", "/d/z.txt").unwrap();
        assert_eq!(db.tagged(Some(2)).unwrap()[0].path, "/d/z.txt");
        // A renamed folder takes the tags inside it along; a similar name does not.
        db.set_tag("/d/subway.txt", 4).unwrap();
        db.move_tag("/d/sub", "/d/renamed").unwrap();
        assert_eq!(db.tagged(Some(3)).unwrap()[0].path, "/d/renamed/b.txt");
        assert_eq!(db.tagged(Some(4)).unwrap()[0].path, "/d/subway.txt");
        db.set_tag("/d/z.txt", 0).unwrap();
        assert!(db.tagged(Some(2)).unwrap().is_empty());
    }

    /// Tags are found and moved through ranges of the path: names next to the folder's
    /// own in sort order (before and after the separator) stay out of it.
    #[test]
    fn tags_of_a_folder_and_its_neighbours() {
        let sep = std::path::MAIN_SEPARATOR;
        let p = |s: &str| s.replace('/', std::path::MAIN_SEPARATOR_STR);
        let db = Db::open_in_memory().unwrap();
        let all = [
            "/d/a.txt",
            "/d/\u{17e} \u{10d}.txt",
            "/d/sub/b.txt",
            "/d/sub/deep/c.txt",
            "/d-x/e.txt",
            "/d.txt",
            "/d0",
            "/da/f.txt",
            "/d",
        ];
        db.set_tags(&all.map(p), 5).unwrap();
        let names = |dir: &str| {
            let mut v: Vec<String> = db
                .tags_in(&p(dir))
                .unwrap()
                .into_iter()
                .map(|t| t.path)
                .collect();
            v.sort();
            v
        };
        assert_eq!(names("/d"), [p("/d/a.txt"), p("/d/\u{17e} \u{10d}.txt")]);
        assert_eq!(names(&format!("/d{sep}")), names("/d"));
        assert_eq!(names("/d/sub"), [p("/d/sub/b.txt")]);
        // Renaming the folder takes what is inside along, at any depth, and nothing else.
        db.move_tag(&p("/d"), &p("/moved")).unwrap();
        let mut now: Vec<String> = db
            .tagged(None)
            .unwrap()
            .into_iter()
            .map(|t| t.path)
            .collect();
        now.sort();
        let mut expected = [
            "/moved/a.txt",
            "/moved/\u{17e} \u{10d}.txt",
            "/moved/sub/b.txt",
            "/moved/sub/deep/c.txt",
            "/d-x/e.txt",
            "/d.txt",
            "/d0",
            "/da/f.txt",
            "/moved",
        ]
        .map(p);
        expected.sort();
        assert_eq!(now, expected);
        // Removing many at once.
        db.set_tags(&expected, 0).unwrap();
        assert!(db.tagged(None).unwrap().is_empty());
    }

    #[test]
    fn wal_stays_bounded_and_truncates_on_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("gall.db");
        let wal = dir.path().join("gall.db-wal");

        let db = Db::open(&file).unwrap();
        for i in 0..300 {
            db.record_visit(&format!("/folder/{i:04}"), 50).unwrap();
        }
        let wal_len = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
        assert!(
            wal_len <= 524_288,
            "WAL grew too large during repeated visits: {wal_len} bytes"
        );
        db.checkpoint();
        let after_ckpt = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
        assert_eq!(after_ckpt, 0, "checkpoint must truncate the WAL to 0 bytes");
        drop(db);

        // Simulate an oversized WAL left on disk by an older version or unclean exit:
        // write > 256 KiB of WAL frames without checkpointing, snapshot the db + wal
        // pair to a fresh path with no competing connections, and verify Db::open shrinks it.
        let legacy = dir.path().join("legacy.db");
        let legacy_wal = dir.path().join("legacy.db-wal");
        {
            let raw = Connection::open(&file).unwrap();
            raw.execute_batch("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;")
                .unwrap();
            for i in 0..120 {
                raw.execute(
                    "INSERT INTO history(path, visited) VALUES(?1, ?2) ON CONFLICT(path) DO UPDATE SET visited = excluded.visited",
                    params![format!("/legacy/{i:04}"), i as i64],
                )
                .unwrap();
            }
            assert!(
                std::fs::metadata(&wal).unwrap().len() > 262_144,
                "precondition: oversized legacy WAL"
            );
            std::fs::copy(&file, &legacy).unwrap();
            std::fs::copy(&wal, &legacy_wal).unwrap();
        }
        let db = Db::open(&legacy).unwrap();
        assert_eq!(
            std::fs::metadata(&legacy_wal).map(|m| m.len()).unwrap_or(0),
            0,
            "Db::open must shrink an oversized pre-existing WAL"
        );
        assert!(!db.history(10).unwrap().is_empty());
    }
}
