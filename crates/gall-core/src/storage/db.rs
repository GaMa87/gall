//! SQLite store: per-folder preferences (V-02, F-02, F-03), folder history
//! (S-04), colour tags (O-14) and recent destinations (N-11).

use crate::error::Result;
use crate::view::sort::SortKey;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

pub struct Db {
    conn: Mutex<Connection>,
}

/// Explicit per-folder settings; `None` = not set for this folder.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderPrefs {
    pub sort_key: Option<SortKey>,
    pub descending: Option<bool>,
    pub folders_on_top: Option<bool>,
    pub view: Option<String>,
}

/// Effective preferences after inheritance and defaults.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPrefs {
    pub sort_key: SortKey,
    pub descending: bool,
    pub folders_on_top: bool,
    pub view: String,
    /// Folder the view was inherited from (None = default).
    pub view_source: Option<String>,
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
"#;

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

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    pub fn get_prefs(&self, path: &str) -> Result<FolderPrefs> {
        let c = self.conn.lock().unwrap();
        let r = c
            .query_row(
                "SELECT sort_key, descending, folders_on_top, view FROM folder_prefs WHERE path = ?1",
                params![path],
                |r| {
                    Ok(FolderPrefs {
                        sort_key: r.get::<_, Option<String>>(0)?.as_deref().and_then(key_from_str),
                        descending: r.get(1)?,
                        folders_on_top: r.get(2)?,
                        view: r.get(3)?,
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
            "INSERT INTO folder_prefs (path, sort_key, descending, folders_on_top, view) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET
               sort_key = COALESCE(excluded.sort_key, sort_key),
               descending = COALESCE(excluded.descending, descending),
               folders_on_top = COALESCE(excluded.folders_on_top, folders_on_top),
               view = COALESCE(excluded.view, view)",
            params![path, p.sort_key.map(key_to_str), p.descending, p.folders_on_top, p.view],
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
    /// folders-on-top inherit from the nearest ancestor that sets them (`M:fileviews`).
    pub fn resolve_prefs(
        &self,
        path: &Path,
        default_sort: SortKey,
        default_desc: bool,
        default_fot: bool,
        default_view: &str,
    ) -> Result<ResolvedPrefs> {
        let own = self.get_prefs(&path.to_string_lossy())?;
        let mut res = ResolvedPrefs {
            sort_key: own.sort_key.unwrap_or(default_sort),
            descending: own.descending.unwrap_or(default_desc),
            folders_on_top: default_fot,
            view: default_view.to_string(),
            view_source: None,
        };
        let (mut got_view, mut got_fot) = (false, false);
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
            if !got_fot {
                if let Some(f) = p.folders_on_top {
                    res.folders_on_top = f;
                    got_fot = true;
                }
            }
            if got_view && got_fot {
                break;
            }
        }
        Ok(res)
    }

    pub fn record_visit(&self, path: &str, keep: u32) -> Result<()> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO history(path, visited) VALUES(?1, ?2) ON CONFLICT(path) DO UPDATE SET visited = excluded.visited",
            params![path, now()],
        )?;
        c.execute(
            "DELETE FROM history WHERE path NOT IN (SELECT path FROM history ORDER BY visited DESC LIMIT ?1)",
            params![keep.max(1)],
        )?;
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

    /// Tags of direct children of `dir` (for the file list).
    pub fn tags_in(&self, dir: &str) -> Result<Vec<Tagged>> {
        let sep = std::path::MAIN_SEPARATOR;
        let prefix = if dir.ends_with(sep) {
            dir.to_string()
        } else {
            format!("{dir}{sep}")
        };
        let c = self.conn.lock().unwrap();
        let mut st =
            c.prepare("SELECT path, color FROM tags WHERE substr(path, 1, length(?1)) = ?1")?;
        let rows = st.query_map(params![prefix], |r| {
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
        for table in ["tags", "folder_prefs"] {
            c.execute(
                &format!(
                    "UPDATE OR REPLACE {table} SET path = ?2 || substr(path, length(?1) + 1) \
                     WHERE path = ?1 OR substr(path, 1, length(?1) + 1) = ?1 || ?3"
                ),
                params![from, to, std::path::MAIN_SEPARATOR_STR],
            )?;
        }
        Ok(())
    }

    pub fn record_destination(&self, path: &str, keep: u32) -> Result<()> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO destinations(path, used) VALUES(?1, ?2) ON CONFLICT(path) DO UPDATE SET used = excluded.used",
            params![path, now()],
        )?;
        c.execute(
            "DELETE FROM destinations WHERE path NOT IN (SELECT path FROM destinations ORDER BY used DESC LIMIT ?1)",
            params![keep.max(1)],
        )?;
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
                folders_on_top: Some(false),
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
            .resolve_prefs(Path::new("/a/b/c"), SortKey::Name, false, true, "default")
            .unwrap();
        assert_eq!(r.view, "names");
        assert_eq!(r.view_source.as_deref(), Some("/a/b"));
        assert!(!r.folders_on_top, "inherited from root");
        assert_eq!(r.sort_key, SortKey::Name, "sort is not inherited");

        let r = db
            .resolve_prefs(Path::new("/a"), SortKey::Name, false, true, "default")
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
}
