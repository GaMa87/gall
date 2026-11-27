//! Sorting (F-02, F-03) and grouping by type. Mirrored in the UI (`ui/src/core/sort.ts`,
//! `sortGroup.ts`) and verified against the same cases (`ui/src/core/sortGroupCases.json`).

use crate::fs::FileEntry;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;

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
    // Composed and decomposed spellings of a name sort together (see `util::unicode`).
    let (ca, cb) = (crate::util::unicode::nfc(a), crate::util::unicode::nfc(b));
    let (mut ai, mut bi) = (ca.chars().peekable(), cb.chars().peekable());
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

/// What is collected before the sort orders the entries of each group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum GroupMode {
    None,
    /// Folders before files, without headers.
    Folders,
    /// By type; folders are a group like the others.
    Type,
    /// Folders, then the files by type. For new installs; older ones keep what they had.
    #[default]
    FoldersType,
}

impl GroupMode {
    pub fn from_folders_on_top(on: bool) -> Self {
        if on {
            GroupMode::Folders
        } else {
            GroupMode::None
        }
    }

    pub fn folders_first(self) -> bool {
        matches!(self, GroupMode::Folders | GroupMode::FoldersType)
    }

    pub fn by_type(self) -> bool {
        matches!(self, GroupMode::Type | GroupMode::FoldersType)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            GroupMode::None => "none",
            GroupMode::Folders => "folders",
            GroupMode::Type => "type",
            GroupMode::FoldersType => "foldersType",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [
            GroupMode::None,
            GroupMode::Folders,
            GroupMode::Type,
            GroupMode::FoldersType,
        ]
        .into_iter()
        .find(|mode| mode.as_str() == text)
    }
}

/// Which group comes first. The direction of the sort never changes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum GroupOrder {
    /// By extension, then dotfiles, files without an extension and folders.
    #[default]
    Name,
    /// Most files first.
    Count,
    /// The most recently changed first.
    Recent,
    /// The largest total size first.
    Size,
}

impl GroupOrder {
    pub fn as_str(self) -> &'static str {
        match self {
            GroupOrder::Name => "name",
            GroupOrder::Count => "count",
            GroupOrder::Recent => "recent",
            GroupOrder::Size => "size",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [
            GroupOrder::Name,
            GroupOrder::Count,
            GroupOrder::Recent,
            GroupOrder::Size,
        ]
        .into_iter()
        .find(|order| order.as_str() == text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GroupSpec {
    pub mode: GroupMode,
    pub order: GroupOrder,
    /// Images, documents, … instead of one group for each extension.
    pub by_kind: bool,
}

/// One group of a sorted listing; its entries follow those of the group before it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    /// `dir`, `ext:png`, `dot` (`.bashrc`), `none` (no extension), `kind:image` or `rest`.
    pub id: String,
    pub count: usize,
    /// Bytes of its files.
    pub size: u64,
    /// The newest change in it.
    pub latest: Option<i64>,
    /// How many types it holds: more than 1 only in `rest`.
    pub types: usize,
}

/// Extensions with a group of their own; the rarest ones beyond them share `rest`, the last.
pub const MAX_TYPE_GROUPS: usize = 200;

/// The group of an entry. The extension is the one the Type column shows: lower case, the part
/// after the last dot (`a.tar.gz` is `gz`); a leading dot alone makes a dotfile and a trailing
/// dot no extension.
pub fn group_id(e: &FileEntry, by_kind: bool) -> Cow<'static, str> {
    if e.is_dir {
        return "dir".into();
    }
    if by_kind {
        return format!("kind:{}", crate::preview::kind_group(&e.name, &e.ext)).into();
    }
    if !e.ext.is_empty() {
        return format!("ext:{}", e.ext).into();
    }
    if e.name.starts_with('.') {
        "dot".into()
    } else {
        "none".into()
    }
}

/// Where a group stands before the chosen order: folders first when they are, the tail last.
fn group_place(id: &str, folders_first: bool) -> u8 {
    match id {
        "dir" if folders_first => 0,
        "rest" => 2,
        _ => 1,
    }
}

fn group_name_cmp(a: &str, b: &str) -> Ordering {
    let class = |id: &str| match id {
        "dot" => 1,
        "none" | "kind:other" => 2,
        "dir" => 3,
        _ => 0,
    };
    class(a).cmp(&class(b)).then_with(|| a.cmp(b))
}

/// Sorts a listing group by group and returns the groups in order; none when the mode does not
/// group by type. The key of an entry is computed once, with the totals, and the sort runs
/// inside each group, so it never compares groups.
pub fn group_entries(v: &mut Vec<FileEntry>, spec: &SortSpec, group: &GroupSpec) -> Vec<Group> {
    if !group.mode.by_type() {
        let spec = SortSpec {
            folders_on_top: group.mode.folders_first(),
            ..*spec
        };
        sort_entries(v, &spec);
        return Vec::new();
    }
    let mut seen: HashMap<Cow<'static, str>, usize> = HashMap::new();
    let mut groups: Vec<Group> = Vec::new();
    let mut members: Vec<Vec<FileEntry>> = Vec::new();
    for e in v.drain(..) {
        let id = group_id(&e, group.by_kind);
        let at = *seen.entry(id).or_insert_with_key(|id| {
            groups.push(Group {
                id: id.to_string(),
                count: 0,
                size: 0,
                latest: None,
                types: 1,
            });
            members.push(Vec::new());
            groups.len() - 1
        });
        let g = &mut groups[at];
        g.count += 1;
        if !e.is_dir {
            g.size += e.size;
        }
        g.latest = g.latest.max(e.modified);
        members[at].push(e);
    }
    let mut all: Vec<(Group, Vec<FileEntry>)> = groups.into_iter().zip(members).collect();
    merge_tail(&mut all);
    let folders_first = group.mode.folders_first();
    all.sort_by(|(a, _), (b, _)| {
        group_place(&a.id, folders_first)
            .cmp(&group_place(&b.id, folders_first))
            .then_with(|| match group.order {
                GroupOrder::Name => Ordering::Equal,
                GroupOrder::Count => b.count.cmp(&a.count),
                GroupOrder::Recent => b.latest.cmp(&a.latest),
                GroupOrder::Size => b.size.cmp(&a.size),
            })
            .then_with(|| group_name_cmp(&a.id, &b.id))
    });
    all.into_iter()
        .map(|(g, mut entries)| {
            sort_entries(&mut entries, spec);
            v.append(&mut entries);
            g
        })
        .collect()
}

/// Thousands of extensions would be thousands of headers: the rarest share one group.
fn merge_tail(all: &mut Vec<(Group, Vec<FileEntry>)>) {
    if all.len() <= MAX_TYPE_GROUPS {
        return;
    }
    let (mut types, mut kept): (Vec<_>, Vec<_>) = std::mem::take(all)
        .into_iter()
        .partition(|(g, _)| g.id.starts_with("ext:"));
    if types.len() > MAX_TYPE_GROUPS {
        types.sort_by(|(a, _), (b, _)| b.count.cmp(&a.count).then_with(|| a.id.cmp(&b.id)));
        let mut rest = Group {
            id: "rest".into(),
            count: 0,
            size: 0,
            latest: None,
            types: 0,
        };
        let mut entries = Vec::new();
        for (g, mut own) in types.drain(MAX_TYPE_GROUPS..) {
            rest.count += g.count;
            rest.size += g.size;
            rest.latest = rest.latest.max(g.latest);
            rest.types += 1;
            entries.append(&mut own);
        }
        kept.push((rest, entries));
    }
    kept.append(&mut types);
    *all = kept;
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

    /// The same file drives `ui/src/core/sortGroup.test.ts`.
    const CASES: &str = include_str!("../../../../ui/src/core/sortGroupCases.json");

    #[derive(Deserialize)]
    struct Cases {
        classify: Vec<(String, bool, String, String)>,
        listing: Vec<(String, bool, u64, i64)>,
        orders: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        title: String,
        sort: SortSpec,
        group: GroupSpec,
        names: Vec<String>,
        groups: Vec<(String, usize, u64)>,
    }

    #[test]
    fn groups_follow_the_shared_cases() {
        let cases: Cases = serde_json::from_str(CASES).unwrap();
        for (name, dir, by_type, by_kind) in &cases.classify {
            let entry = e(name, *dir, 1, 1);
            assert_eq!(group_id(&entry, false), by_type.as_str(), "{name}");
            assert_eq!(group_id(&entry, true), by_kind.as_str(), "{name}");
        }
        for case in &cases.orders {
            // The order of the listing must not matter.
            for reversed in [false, true] {
                let mut v: Vec<_> = cases
                    .listing
                    .iter()
                    .map(|(name, dir, size, modified)| e(name, *dir, *size, *modified))
                    .collect();
                if reversed {
                    v.reverse();
                }
                let groups: Vec<_> = group_entries(&mut v, &case.sort, &case.group)
                    .into_iter()
                    .map(|g| (g.id, g.count, g.size))
                    .collect();
                assert_eq!(names(&v), case.names, "{}", case.title);
                assert_eq!(groups, case.groups, "{}", case.title);
            }
        }
    }

    #[test]
    fn the_rarest_of_thousands_of_types_share_the_last_group() {
        let mut v: Vec<_> = (0..3000)
            .map(|i| e(&format!("f.x{i:04}"), false, 1, i))
            .collect();
        v.extend((0..5).map(|i| e(&format!("n{i}.zzz"), false, 1, 0)));
        v.push(e("dir", true, 0, 0));
        v.push(e("plain", false, 1, 0));
        for order in [GroupOrder::Name, GroupOrder::Count, GroupOrder::Recent] {
            let mut v = v.clone();
            let spec = GroupSpec {
                order,
                ..Default::default()
            };
            let groups = group_entries(&mut v, &SortSpec::default(), &spec);
            assert_eq!(groups.len(), MAX_TYPE_GROUPS + 3, "{order:?}");
            assert_eq!(groups[0].id, "dir");
            assert!(groups.iter().any(|g| g.id == "ext:zzz" && g.count == 5));
            let rest = groups.last().unwrap();
            assert_eq!((rest.id.as_str(), rest.count), ("rest", 2801));
            assert_eq!(rest.types, 2801);
            assert_eq!(groups.iter().map(|g| g.count).sum::<usize>(), v.len());
        }
        // Sorted as one group, not type by type.
        let groups = group_entries(&mut v, &SortSpec::default(), &GroupSpec::default());
        let tail = &v[v.len() - groups.last().unwrap().count..];
        assert!(tail
            .windows(2)
            .all(|w| natural_cmp(&w[0].name, &w[1].name) == Ordering::Less));
    }

    #[test]
    fn composed_and_decomposed_names_sort_together() {
        // Without normalization NFD "č" sorted as "c" + a mark (before "da") and NFC "č"
        // after "z": one folder, two places. Both spellings now sort the same way.
        for other in ["ba", "ca", "da", "z", "zz"] {
            assert_eq!(
                natural_cmp("c\u{30c}a", other),
                natural_cmp("ča", other),
                "{other}"
            );
        }
    }
}
