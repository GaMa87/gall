//! Tabs, browsers and the per-window session (T-01…T-11, W-02…W-06).

use super::history::History;
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "camelCase")]
pub enum Layout {
    /// Browsers side by side, Miller columns on top of each. (W-02)
    #[default]
    Standard,
    /// Browsers stacked, current folder is the last column. (W-03)
    Columns,
    /// Single browser, big list + preview on the right. (W-04)
    Classic,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    pub id: u64,
    pub path: String,
    #[serde(default)]
    pub alias: Option<String>,
    /// Keep the alias when navigating away (otherwise it is cleared). (T-04)
    #[serde(default)]
    pub keep_alias: bool,
    #[serde(default)]
    pub history: History,
    /// Epoch ms of the last navigation/activation; drives "close stale". (T-06)
    #[serde(default)]
    pub last_used: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Browser {
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// Recently closed tabs, most recent last. (T-03)
    #[serde(default)]
    pub closed: Vec<Tab>,
    /// Collapsed browsers keep their tabs and come back exactly as they were. (W-06)
    #[serde(default)]
    pub collapsed: bool,
    /// Preview & details pane of this browser; None = the `showDetailsPane` setting.
    #[serde(default)]
    pub details: Option<bool>,
    /// Miller columns (true) or a breadcrumb bar (false) in this browser, chosen per
    /// layout (turning them off in Standard must not undo the Columns layout). A layout
    /// not in the map uses its default: the `showNavColumns` setting in Standard,
    /// columns in Columns, breadcrumbs in Classic.
    #[serde(default, deserialize_with = "columns_compat")]
    pub columns: std::collections::BTreeMap<Layout, bool>,
    /// Sort and view of this browser in every folder it opens, used while folders do not
    /// remember their own (`rememberFolderViews` off); unset fields = the defaults.
    #[serde(default)]
    pub view: crate::storage::db::FolderPrefs,
}

/// Reads `columns` as a map, or as the single value a build of 0.1.2 saved (it meant the
/// Standard layout).
fn columns_compat<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<std::collections::BTreeMap<Layout, bool>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Saved {
        Map(std::collections::BTreeMap<Layout, bool>),
        One(Option<bool>),
    }
    Ok(match Saved::deserialize(d)? {
        Saved::Map(m) => m,
        Saved::One(Some(on)) => [(Layout::Standard, on)].into(),
        Saved::One(None) => Default::default(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum NewTabPlacement {
    #[default]
    AfterCurrent,
    End,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub name: Option<String>,
    pub layout: Layout,
    pub browsers: Vec<Browser>,
    pub active_browser: usize,
    #[serde(default)]
    next_id: u64,
}

const MAX_CLOSED: usize = 25;
pub const STALE_MS: i64 = 24 * 3600 * 1000;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Session {
    /// A fresh two-browser session opened at `left` and `right`.
    pub fn new(left: &str, right: &str) -> Self {
        let mut s = Session {
            name: None,
            layout: Layout::Standard,
            browsers: vec![],
            active_browser: 0,
            next_id: 1,
        };
        for p in [left, right] {
            let t = s.make_tab(p);
            s.browsers.push(Browser {
                tabs: vec![t],
                active: 0,
                closed: vec![],
                collapsed: false,
                details: None,
                columns: Default::default(),
                view: Default::default(),
            });
        }
        s
    }

    fn make_tab(&mut self, path: &str) -> Tab {
        let id = self.next_id;
        self.next_id += 1;
        Tab {
            id,
            path: path.to_string(),
            alias: None,
            keep_alias: false,
            history: History::default(),
            last_used: now_ms(),
        }
    }

    /// Repairs invariants after loading from disk.
    pub fn sanitize(&mut self, fallback: &str) {
        while self.browsers.len() < 2 {
            let t = self.make_tab(fallback);
            self.browsers.push(Browser {
                tabs: vec![t],
                active: 0,
                closed: vec![],
                collapsed: false,
                details: None,
                columns: Default::default(),
                view: Default::default(),
            });
        }
        self.browsers.truncate(2);
        let max_id = self
            .browsers
            .iter()
            .flat_map(|b| b.tabs.iter().chain(b.closed.iter()))
            .map(|t| t.id)
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(max_id + 1);
        for i in 0..self.browsers.len() {
            if self.browsers[i].tabs.is_empty() {
                let t = self.make_tab(fallback);
                self.browsers[i].tabs.push(t);
            }
            let b = &mut self.browsers[i];
            b.active = b.active.min(b.tabs.len() - 1);
        }
        self.active_browser = self.active_browser.min(1);
    }

    pub fn browser(&self, b: usize) -> &Browser {
        &self.browsers[b.min(self.browsers.len() - 1)]
    }

    fn bm(&mut self, b: usize) -> &mut Browser {
        let i = b.min(self.browsers.len() - 1);
        &mut self.browsers[i]
    }

    pub fn active_tab(&self, b: usize) -> &Tab {
        let br = self.browser(b);
        &br.tabs[br.active]
    }

    fn active_tab_mut(&mut self, b: usize) -> &mut Tab {
        let br = self.bm(b);
        let a = br.active;
        &mut br.tabs[a]
    }

    /// Navigates the active tab of browser `b` to `path`, recording history.
    pub fn navigate(&mut self, b: usize, path: &str) {
        let t = self.active_tab_mut(b);
        if t.path == path {
            return;
        }
        let from = std::mem::replace(&mut t.path, path.to_string());
        t.history.visit(&from);
        if !t.keep_alias {
            t.alias = None;
        }
        t.last_used = now_ms();
    }

    pub fn back(&mut self, b: usize) -> bool {
        let t = self.active_tab_mut(b);
        match t.history.back(&t.path.clone()) {
            Some(p) => {
                t.path = p;
                t.last_used = now_ms();
                true
            }
            None => false,
        }
    }

    pub fn forward(&mut self, b: usize) -> bool {
        let t = self.active_tab_mut(b);
        match t.history.forward(&t.path.clone()) {
            Some(p) => {
                t.path = p;
                t.last_used = now_ms();
                true
            }
            None => false,
        }
    }

    /// Opens a new tab. `path = None` clones the current tab's path (T-01).
    pub fn new_tab(
        &mut self,
        b: usize,
        path: Option<&str>,
        placement: NewTabPlacement,
        activate: bool,
    ) -> u64 {
        let p = path
            .map(str::to_string)
            .unwrap_or_else(|| self.active_tab(b).path.clone());
        let t = self.make_tab(&p);
        let id = t.id;
        let br = self.bm(b);
        let idx = match placement {
            NewTabPlacement::AfterCurrent => br.active + 1,
            NewTabPlacement::End => br.tabs.len(),
        };
        br.tabs.insert(idx, t);
        if activate {
            br.active = idx;
        }
        id
    }

    pub fn activate(&mut self, b: usize, idx: usize) {
        let br = self.bm(b);
        if idx < br.tabs.len() {
            br.active = idx;
            br.tabs[idx].last_used = now_ms();
        }
        self.active_browser = b.min(1);
    }

    /// Closes a tab. The last tab of a browser is never removed; it would leave
    /// the browser empty, so it is kept (the UI may collapse the browser instead).
    pub fn close_tab(&mut self, b: usize, idx: usize) -> bool {
        let br = self.bm(b);
        if br.tabs.len() <= 1 || idx >= br.tabs.len() {
            return false;
        }
        let t = br.tabs.remove(idx);
        br.closed.push(t);
        if br.closed.len() > MAX_CLOSED {
            br.closed.remove(0);
        }
        if br.active > idx || br.active >= br.tabs.len() {
            br.active = br.active.saturating_sub(1);
        }
        true
    }

    pub fn close_others(&mut self, b: usize, keep: usize) {
        let n = self.browser(b).tabs.len();
        for i in (0..n).rev() {
            if i != keep {
                self.close_tab(b, i);
            }
        }
        self.bm(b).active = 0;
    }

    /// Closes tabs not used for 24 h, except the active one. Returns count.
    pub fn close_stale(&mut self, b: usize, now: i64) -> usize {
        let active_id = self.active_tab(b).id;
        let stale: Vec<usize> = self
            .browser(b)
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.id != active_id && now - t.last_used > STALE_MS)
            .map(|(i, _)| i)
            .collect();
        for &i in stale.iter().rev() {
            self.close_tab(b, i);
        }
        stale.len()
    }

    pub fn reopen_closed(&mut self, b: usize) -> bool {
        let br = self.bm(b);
        match br.closed.pop() {
            Some(mut t) => {
                t.last_used = now_ms();
                br.tabs.push(t);
                br.active = br.tabs.len() - 1;
                true
            }
            None => false,
        }
    }

    /// Reorders within a browser.
    pub fn move_tab_within(&mut self, b: usize, from: usize, to: usize) {
        let br = self.bm(b);
        if from >= br.tabs.len() {
            return;
        }
        let active_id = br.tabs[br.active].id;
        let t = br.tabs.remove(from);
        let to = to.min(br.tabs.len());
        br.tabs.insert(to, t);
        br.active = br.tabs.iter().position(|t| t.id == active_id).unwrap_or(0);
    }

    /// Moves (or copies) a tab to the other browser (T-05).
    pub fn transfer_tab(&mut self, from_b: usize, idx: usize, copy: bool) {
        let to_b = 1 - from_b.min(1);
        let Some(src) = self.browser(from_b).tabs.get(idx).cloned() else {
            return;
        };
        let mut t = src;
        if copy {
            t.id = self.next_id;
            self.next_id += 1;
        } else if !self.close_tab(from_b, idx) {
            // Moving the only tab: leave a clone behind so the browser is not empty.
            t.id = self.next_id;
            self.next_id += 1;
        }
        let dst = self.bm(to_b);
        dst.tabs.push(t);
        dst.active = dst.tabs.len() - 1;
        self.active_browser = to_b;
    }

    /// Swaps the active tabs of the two browsers (path, history, alias), while each
    /// browser keeps its own layout: preview pane, other tabs, collapsed state.
    pub fn swap_active_tabs(&mut self) {
        if self.browsers.len() < 2 {
            return;
        }
        let (l, r) = self.browsers.split_at_mut(1);
        let (a, b) = (l[0].active, r[0].active);
        std::mem::swap(&mut l[0].tabs[a], &mut r[0].tabs[b]);
        let now = now_ms();
        l[0].tabs[a].last_used = now;
        r[0].tabs[b].last_used = now;
    }

    pub fn set_alias(&mut self, b: usize, idx: usize, alias: Option<String>, keep: bool) {
        if let Some(t) = self.bm(b).tabs.get_mut(idx) {
            t.alias = alias.filter(|a| !a.trim().is_empty());
            t.keep_alias = keep && t.alias.is_some();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_and_history() {
        let mut s = Session::new("/a", "/z");
        s.navigate(0, "/a/b");
        s.navigate(0, "/a/b/c");
        assert!(s.back(0));
        assert_eq!(s.active_tab(0).path, "/a/b");
        assert!(s.forward(0));
        assert_eq!(s.active_tab(0).path, "/a/b/c");
        assert_eq!(s.active_tab(1).path, "/z", "browsers are independent");
    }

    #[test]
    fn tabs_lifecycle() {
        let mut s = Session::new("/a", "/z");
        s.new_tab(0, None, NewTabPlacement::AfterCurrent, true);
        assert_eq!(s.browser(0).tabs.len(), 2);
        assert_eq!(s.active_tab(0).path, "/a", "new tab clones current");
        s.navigate(0, "/b");
        assert!(s.close_tab(0, 1));
        assert_eq!(s.browser(0).active, 0);
        assert!(!s.close_tab(0, 0), "last tab stays");
        assert!(s.reopen_closed(0));
        assert_eq!(s.active_tab(0).path, "/b");
    }

    #[test]
    fn alias_cleared_unless_kept() {
        let mut s = Session::new("/a", "/z");
        s.set_alias(0, 0, Some("Work".into()), false);
        s.navigate(0, "/b");
        assert_eq!(s.active_tab(0).alias, None);
        s.set_alias(0, 0, Some("Work".into()), true);
        s.navigate(0, "/c");
        assert_eq!(s.active_tab(0).alias.as_deref(), Some("Work"));
    }

    #[test]
    fn transfer_and_stale() {
        let mut s = Session::new("/a", "/z");
        s.new_tab(0, Some("/q"), NewTabPlacement::End, false);
        s.transfer_tab(0, 1, false);
        assert_eq!(s.browser(0).tabs.len(), 1);
        assert_eq!(s.browser(1).tabs.len(), 2);
        assert_eq!(s.active_tab(1).path, "/q");
        s.transfer_tab(0, 0, false); // only tab: clone left behind
        assert_eq!(s.browser(0).tabs.len(), 1);

        let mut s = Session::new("/a", "/z");
        s.new_tab(0, Some("/old"), NewTabPlacement::End, false);
        s.browsers[0].tabs[1].last_used = 0;
        assert_eq!(s.close_stale(0, now_ms()), 1);
    }

    #[test]
    fn reorder_keeps_active() {
        let mut s = Session::new("/a", "/z");
        s.new_tab(0, Some("/b"), NewTabPlacement::End, false);
        s.new_tab(0, Some("/c"), NewTabPlacement::End, true);
        s.move_tab_within(0, 2, 0);
        assert_eq!(s.active_tab(0).path, "/c");
        assert_eq!(s.browser(0).active, 0);
    }

    #[test]
    fn swap_moves_tabs_not_panes() {
        let mut s = Session::new("/a", "/z");
        s.navigate(0, "/a/b");
        s.browsers[1].details = Some(true);
        s.browsers[0].columns.insert(Layout::Standard, false);
        s.swap_active_tabs();
        assert_eq!(s.active_tab(0).path, "/z");
        assert_eq!(s.active_tab(1).path, "/a/b");
        assert!(s.back(1), "history travels with the tab");
        assert_eq!(s.active_tab(1).path, "/a");
        assert_eq!(
            s.browsers[1].details,
            Some(true),
            "preview stays on its side"
        );
        assert_eq!(s.browsers[0].details, None);
        assert_eq!(
            s.browsers[0].columns.get(&Layout::Standard),
            Some(&false),
            "breadcrumbs stay on their side"
        );
        assert!(s.browsers[1].columns.is_empty());
    }

    /// Sessions saved before columns were per browser load with the setting's default.
    #[test]
    fn old_sessions_load_without_columns() {
        let mut v = serde_json::to_value(Session::new("/a", "/z")).unwrap();
        for b in v["browsers"].as_array_mut().unwrap() {
            b.as_object_mut().unwrap().remove("columns");
        }
        let s: Session = serde_json::from_value(v.clone()).unwrap();
        assert!(s.browsers[0].columns.is_empty());
        // A 0.1.2 build saved one value for the Standard layout.
        v["browsers"][0]["columns"] = serde_json::json!(false);
        v["browsers"][1]["columns"] = serde_json::Value::Null;
        let s: Session = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(s.browsers[0].columns.get(&Layout::Standard), Some(&false));
        assert!(s.browsers[1].columns.is_empty());
        // Now one value per layout, and back.
        v["browsers"][0]["columns"] = serde_json::json!({ "classic": true, "columns": false });
        let s: Session = serde_json::from_value(v).unwrap();
        assert_eq!(s.browsers[0].columns.get(&Layout::Classic), Some(&true));
        let again: Session = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(again, s);
    }

    #[test]
    fn sanitize_repairs() {
        let mut s = Session::new("/a", "/z");
        s.browsers.pop();
        s.browsers[0].active = 9;
        s.sanitize("/home");
        assert_eq!(s.browsers.len(), 2);
        assert_eq!(s.browsers[0].active, 0);
    }
}
