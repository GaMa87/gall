//! Favourites groups (S-03).

use serde::{Deserialize, Serialize};

pub const TRASH_URI: &str = "trash:///";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Favorite {
    pub path: String,
    #[serde(default)]
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteGroup {
    pub id: u64,
    pub name: String,
    pub items: Vec<Favorite>,
    #[serde(default)]
    pub collapsed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Favorites {
    pub groups: Vec<FavoriteGroup>,
    #[serde(default)]
    next_id: u64,
}

impl Favorites {
    /// The default group built from the user's standard folders.
    pub fn default_group_items() -> Vec<Favorite> {
        let mut v = vec![];
        let mut add = |p: Option<std::path::PathBuf>| {
            if let Some(p) = p.filter(|p| p.is_dir()) {
                let s = p.to_string_lossy().into_owned();
                if !v.iter().any(|f: &Favorite| f.path == s) {
                    v.push(Favorite {
                        path: s,
                        alias: None,
                    });
                }
            }
        };
        add(dirs::home_dir());
        add(dirs::desktop_dir());
        add(dirs::document_dir());
        add(dirs::download_dir());
        add(dirs::picture_dir());
        add(dirs::audio_dir());
        add(dirs::video_dir());
        v.push(Favorite {
            path: TRASH_URI.into(),
            alias: Some("Trash".into()),
        });
        v
    }

    pub fn with_defaults() -> Self {
        let mut f = Favorites::default();
        let name = whoami_name();
        let id = f.add_group(&name);
        f.group_mut(id).unwrap().items = Self::default_group_items();
        f
    }

    pub fn sanitize(&mut self) {
        let max = self.groups.iter().map(|g| g.id).max().unwrap_or(0);
        self.next_id = self.next_id.max(max + 1);
    }

    fn group_mut(&mut self, id: u64) -> Option<&mut FavoriteGroup> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    pub fn add_group(&mut self, name: &str) -> u64 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        self.groups.push(FavoriteGroup {
            id,
            name: name.into(),
            items: vec![],
            collapsed: false,
        });
        id
    }

    pub fn rename_group(&mut self, id: u64, name: &str) {
        if let Some(g) = self.group_mut(id) {
            g.name = name.into();
        }
    }

    pub fn delete_group(&mut self, id: u64) {
        // The trash entry can't be lost (M:favorites): re-home it first.
        let trash = self
            .groups
            .iter()
            .find(|g| g.id == id)
            .and_then(|g| g.items.iter().find(|f| f.path == TRASH_URI).cloned());
        self.groups.retain(|g| g.id != id);
        if let Some(t) = trash {
            if self.groups.is_empty() {
                let nid = self.add_group("Favorites");
                self.group_mut(nid).unwrap().items.push(t);
            } else {
                self.groups[0].items.push(t);
            }
        }
    }

    /// Moves a group up (`delta = -1`) or down (`+1`).
    pub fn move_group(&mut self, id: u64, delta: i32) {
        if let Some(i) = self.groups.iter().position(|g| g.id == id) {
            let j = i as i32 + delta;
            if j >= 0 && (j as usize) < self.groups.len() {
                self.groups.swap(i, j as usize);
            }
        }
    }

    pub fn set_collapsed(&mut self, id: u64, collapsed: bool) {
        if let Some(g) = self.group_mut(id) {
            g.collapsed = collapsed;
        }
    }

    /// Adds a path to a group (no duplicates within the group). Returns false if present.
    pub fn add_item(&mut self, id: u64, path: &str, index: Option<usize>) -> bool {
        let Some(g) = self.group_mut(id) else {
            return false;
        };
        if g.items.iter().any(|f| f.path == path) {
            return false;
        }
        let i = index.unwrap_or(g.items.len()).min(g.items.len());
        g.items.insert(
            i,
            Favorite {
                path: path.into(),
                alias: None,
            },
        );
        true
    }

    pub fn remove_item(&mut self, id: u64, index: usize) {
        if let Some(g) = self.group_mut(id) {
            if index < g.items.len() && g.items[index].path != TRASH_URI {
                g.items.remove(index);
            }
        }
    }

    pub fn set_alias(&mut self, id: u64, index: usize, alias: Option<String>) {
        if let Some(f) = self.group_mut(id).and_then(|g| g.items.get_mut(index)) {
            f.alias = alias.filter(|a| !a.trim().is_empty());
        }
    }

    /// Drag-reorder within or between groups.
    pub fn move_item(
        &mut self,
        from_group: u64,
        from_index: usize,
        to_group: u64,
        to_index: usize,
    ) {
        let Some(item) = self
            .group_mut(from_group)
            .and_then(|g| (from_index < g.items.len()).then(|| g.items.remove(from_index)))
        else {
            return;
        };
        let dest_has = self
            .groups
            .iter()
            .find(|g| g.id == to_group)
            .map(|g| g.items.iter().any(|f| f.path == item.path));
        match (self.group_mut(to_group), dest_has) {
            (Some(g), Some(false)) => {
                let i = to_index.min(g.items.len());
                g.items.insert(i, item);
            }
            _ => {
                // Invalid target or duplicate: put it back.
                let g = self.group_mut(from_group).unwrap();
                let i = from_index.min(g.items.len());
                g.items.insert(i, item);
            }
        }
    }

    pub fn restore_default_group(&mut self) -> u64 {
        let id = self.add_group(&whoami_name());
        let items = Self::default_group_items();
        // Avoid a second trash entry.
        let has_trash = self
            .groups
            .iter()
            .any(|g| g.items.iter().any(|f| f.path == TRASH_URI));
        self.group_mut(id).unwrap().items = items
            .into_iter()
            .filter(|f| !(has_trash && f.path == TRASH_URI))
            .collect();
        id
    }
}

fn whoami_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "Favorites".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_and_items() {
        let mut f = Favorites::default();
        let a = f.add_group("A");
        let b = f.add_group("B");
        assert!(f.add_item(a, "/x", None));
        assert!(!f.add_item(a, "/x", None), "no duplicates");
        f.add_item(a, "/y", Some(0));
        assert_eq!(f.groups[0].items[0].path, "/y");
        f.move_item(a, 0, b, 0);
        assert_eq!(f.groups[1].items[0].path, "/y");
        f.move_group(b, -1);
        assert_eq!(f.groups[0].name, "B");
        f.set_alias(b, 0, Some("Why".into()));
        assert_eq!(f.groups[0].items[0].alias.as_deref(), Some("Why"));
    }

    #[test]
    fn trash_survives_group_deletion() {
        let mut f = Favorites::default();
        let a = f.add_group("A");
        let b = f.add_group("B");
        f.add_item(a, TRASH_URI, None);
        f.remove_item(a, 0);
        assert_eq!(f.groups[0].items.len(), 1, "trash cannot be removed");
        f.delete_group(a);
        assert!(f
            .groups
            .iter()
            .any(|g| g.id == b && g.items.iter().any(|i| i.path == TRASH_URI)));
    }
}
