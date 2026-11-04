//! Sorting (F-02, F-03). Mirrored in the UI (`ui/src/core/sort.ts`) and
//! verified against the same cases.

use crate::fs::FileEntry;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum SortKey {
    #[default]
    Name,
    Ext,
    Modified,
    Created,
    Size,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SortSpec {
    pub key: SortKey,
    pub descending: bool,
    pub folders_on_top: bool,
    /// Folders always sort by name regardless of `key` (Settings > View).
    pub folders_by_name: bool,
}

/// Natural, case-insensitive comparison: "file2" < "file10".
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b), // stable tie-break on exact bytes
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let na: String =
                    std::iter::from_fn(|| ai.next_if(|c| c.is_ascii_digit())).collect();
                let nb: String =
                    std::iter::from_fn(|| bi.next_if(|c| c.is_ascii_digit())).collect();
                let ta = na.trim_start_matches('0');
                let tb = nb.trim_start_matches('0');
                let o = ta
                    .len()
                    .cmp(&tb.len())
                    .then_with(|| ta.cmp(tb))
                    .then_with(|| na.len().cmp(&nb.len()));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                let o = x.to_lowercase().cmp(y.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

fn by_key(a: &FileEntry, b: &FileEntry, key: SortKey) -> Ordering {
    let name = || natural_cmp(&a.name, &b.name);
    match key {
        SortKey::Name => name(),
        SortKey::Ext => a.ext.cmp(&b.ext).then_with(name),
        SortKey::Modified => a.modified.cmp(&b.modified).then_with(name),
        SortKey::Created => a.created.cmp(&b.created).then_with(name),
        SortKey::Size => a.size.cmp(&b.size).then_with(name),
    }
}

pub fn compare(a: &FileEntry, b: &FileEntry, spec: &SortSpec) -> Ordering {
    if spec.folders_on_top && a.is_dir != b.is_dir {
        return if a.is_dir {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    // Folders have no intrinsic size; sorting them by size is meaningless (M:folderpane).
    let key = if a.is_dir && b.is_dir && (spec.folders_by_name || spec.key == SortKey::Size) {
        SortKey::Name
    } else {
        spec.key
    };
    let o = by_key(a, b, key);
    if spec.descending && !(a.is_dir && b.is_dir && spec.folders_by_name) {
        o.reverse()
    } else {
        o
    }
}

pub fn sort_entries(v: &mut [FileEntry], spec: &SortSpec) {
    v.sort_by(|a, b| compare(a, b, spec));
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::fs::EntryKind;

    pub fn e(name: &str, dir: bool, size: u64, modified: i64) -> FileEntry {
        FileEntry {
            name: name.into(),
            path: format!("/{name}"),
            kind: if dir { EntryKind::Dir } else { EntryKind::File },
            is_dir: dir,
            is_hidden: false,
            is_symlink: false,
            cloud_only: false,
            mode: None,
            owner: None,
            link_target: None,
            attributes: None,
            size,
            modified: Some(modified),
            created: Some(modified),
            ext: if dir {
                String::new()
            } else {
                crate::fs::entry::extension_of(name)
            },
            readonly: false,
        }
    }

    fn names(v: &[FileEntry]) -> Vec<&str> {
        v.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn natural() {
        let mut v = vec!["file10", "File2", "file1", "file02"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["file1", "File2", "file02", "file10"]);
    }

    #[test]
    fn folders_on_top_and_size() {
        let mut v = vec![
            e("b.txt", false, 10, 1),
            e("Zdir", true, 0, 5),
            e("a.txt", false, 99, 2),
            e("adir", true, 0, 9),
        ];
        let spec = SortSpec {
            key: SortKey::Size,
            descending: true,
            folders_on_top: true,
            folders_by_name: false,
        };
        sort_entries(&mut v, &spec);
        assert_eq!(names(&v), ["Zdir", "adir", "a.txt", "b.txt"]);
    }

    #[test]
    fn folders_by_name_ignores_direction() {
        let mut v = vec![
            e("bdir", true, 0, 9),
            e("adir", true, 0, 1),
            e("x.txt", false, 1, 5),
            e("y.txt", false, 1, 6),
        ];
        let spec = SortSpec {
            key: SortKey::Modified,
            descending: true,
            folders_on_top: true,
            folders_by_name: true,
        };
        sort_entries(&mut v, &spec);
        assert_eq!(names(&v), ["adir", "bdir", "y.txt", "x.txt"]);
    }

    #[test]
    fn mixed_when_not_on_top() {
        let mut v = vec![
            e("b", true, 0, 1),
            e("a.txt", false, 1, 1),
            e("c.txt", false, 1, 1),
        ];
        sort_entries(&mut v, &SortSpec::default());
        assert_eq!(names(&v), ["a.txt", "b", "c.txt"]);
    }
}
