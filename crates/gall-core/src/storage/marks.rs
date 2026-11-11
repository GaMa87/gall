//! Vim-style folder marks: `m{letter}` remembers a folder, `'{letter}` goes back to it.
//!
//! Every mark is global and survives restarts (like Vim's uppercase marks); a file
//! manager has no per-buffer position for lowercase marks to belong to. The special
//! key `'` holds the folder left by the last jump, so `''` returns to it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Key of the automatic "before the last jump" mark.
pub const PREVIOUS: char = '\'';

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
    /// The marked folder.
    pub path: String,
    /// The item focused in it when the mark was set (`` `{letter} `` selects it again).
    #[serde(default)]
    pub item: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Marks {
    /// Mark key (`a`–`z`, `A`–`Z` or `'`) → mark.
    #[serde(default)]
    pub marks: BTreeMap<String, Mark>,
}

/// A key a mark can be stored under: an ASCII letter, or `'` (set by jumps).
pub fn valid_key(key: &str) -> bool {
    let mut c = key.chars();
    matches!((c.next(), c.next()), (Some(k), None) if k.is_ascii_alphabetic() || k == PREVIOUS)
}

impl Marks {
    /// Drops entries a hand-edited or older file may hold under keys that no key reaches.
    pub fn sanitize(&mut self) {
        self.marks
            .retain(|k, m| valid_key(k) && !m.path.trim().is_empty());
    }

    pub fn get(&self, key: &str) -> Option<&Mark> {
        self.marks.get(key)
    }

    /// Stores `path` (and the focused `item`) under `key`. Returns false for an invalid
    /// key or an empty path, which leave the marks unchanged.
    pub fn set(&mut self, key: &str, path: &str, item: Option<String>) -> bool {
        if !valid_key(key) || path.trim().is_empty() {
            return false;
        }
        let item = item.filter(|i| !i.trim().is_empty());
        self.marks.insert(
            key.to_string(),
            Mark {
                path: path.to_string(),
                item,
            },
        );
        true
    }

    pub fn remove(&mut self, key: &str) -> Option<Mark> {
        self.marks.remove(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::JsonStore;

    #[test]
    fn keys() {
        for k in ["a", "z", "A", "Z", "'"] {
            assert!(valid_key(k), "{k}");
        }
        for k in ["", "ab", "1", "`", " ", "č", "é"] {
            assert!(!valid_key(k), "{k:?}");
        }
    }

    #[test]
    fn set_overwrite_remove() {
        let mut m = Marks::default();
        assert!(m.set("a", "/one", Some("/one/x.txt".into())));
        assert!(
            m.set("A", "/two", None),
            "upper and lower case are separate marks"
        );
        assert_eq!(m.get("a").unwrap().item.as_deref(), Some("/one/x.txt"));
        assert!(m.set("a", "/three", Some("  ".into())));
        assert_eq!(
            m.get("a"),
            Some(&Mark {
                path: "/three".into(),
                item: None
            })
        );
        assert_eq!(m.get("A").unwrap().path, "/two");
        assert!(!m.set("1", "/x", None));
        assert!(!m.set("b", " ", None));
        assert_eq!(m.marks.len(), 2);
        assert_eq!(m.remove("a").unwrap().path, "/three");
        assert!(m.get("a").is_none());
    }

    #[test]
    fn survives_a_restart_and_drops_bad_entries() {
        let d = tempfile::tempdir().unwrap();
        let store = JsonStore::new(d.path().join("marks.json"));
        let mut m = Marks::default();
        // Unicode and Windows paths are kept verbatim.
        m.set("w", r"C:\Users\Ana\Dokumenti\čšž", None);
        m.set("'", "/prev", None);
        store.save(&m).unwrap();
        let (back, _): (Marks, _) = store.load();
        assert_eq!(back, m);

        std::fs::write(
            store.path(),
            r#"{"marks":{"a":{"path":"/ok"},"7":{"path":"/no"},"b":{"path":""}}}"#,
        )
        .unwrap();
        let (mut loaded, _): (Marks, _) = store.load();
        loaded.sanitize();
        assert_eq!(loaded.marks.keys().collect::<Vec<_>>(), ["a"]);
        assert_eq!(loaded.get("a").unwrap().item, None);
    }
}
