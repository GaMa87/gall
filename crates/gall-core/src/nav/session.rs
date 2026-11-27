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
    /// Hidden files override for this browser; None = synced with the `showHidden` setting.
    #[serde(default)]
    pub show_hidden: Option<bool>,
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

/// Bridge mode linking a left tab and a right tab inside a workspace session (WN-180).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PairMode {
    /// Activate together + quick swap (`⇄`).
    #[default]
    Linked,
    /// Relative mirrored navigation across both roots (`🔗`).
    Tandem,
    /// Directional copy/move pipeline across the bridge (`→` / `←`).
    Flow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum FlowDirection {
    #[default]
    LeftToRight,
    RightToLeft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum FlowOp {
    #[default]
    Copy,
    Move,
}

/// A live bridge pairing one tab in browser 0 (left/top) with one tab in browser 1 (right/bottom).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TabPair {
    pub id: u64,
    pub left_tab_id: u64,
    pub right_tab_id: u64,
    #[serde(default)]
    pub mode: PairMode,
    /// Root path anchors used by `Tandem` mode to compute relative mirrored paths.
    #[serde(default)]
    pub left_root: String,
    #[serde(default)]
    pub right_root: String,
    #[serde(default)]
    pub flow_dir: FlowDirection,
    #[serde(default)]
    pub flow_op: FlowOp,
    /// Deterministic visual slot (`0..6`) for matching connector accents in both TabBars.
    #[serde(default)]
    pub color_slot: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub name: Option<String>,
    pub layout: Layout,
    pub browsers: Vec<Browser>,
    pub active_browser: usize,
    #[serde(default)]
    pub pairs: Vec<TabPair>,
    /// Recently detached pairs (from tab close) so reopening restores the pair when its partner is still open.
    #[serde(default)]
    closed_pairs: Vec<TabPair>,
    #[serde(default)]
    next_id: u64,
}

const MAX_CLOSED: usize = 25;
pub const STALE_MS: i64 = 24 * 3600 * 1000;
const PAIR_COLOR_SLOTS: u8 = 6;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn allocate_unique_id(next_id: &mut u64, used_ids: &std::collections::HashSet<u64>) -> u64 {
    let mut candidate = (*next_id).max(1);
    loop {
        if !used_ids.contains(&candidate) {
            *next_id = candidate.checked_add(1).unwrap_or(1);
            return candidate;
        }
        candidate = candidate.checked_add(1).unwrap_or(1);
    }
}

/// Computes the mirrored partner path in `Tandem` mode when one side navigates to `target`.
/// Returns `(partner_path, optional_reanchored_roots)`.
///
/// - Navigating inside `src_root` appends the same relative subpath onto `dst_root`.
/// - Navigating strictly above `src_root` by `k` ancestors climbs `dst_root` by `k` parents
///   and re-anchors both roots there so walking back down into sibling folders stays mirrored.
/// - Jumping to an unrelated location outside `src_root` returns `None` so the pair stays
///   intact without forcing an invalid partner jump.
pub fn tandem_mirror_path(
    src_root: &str,
    dst_root: &str,
    target: &str,
) -> Option<(String, Option<(String, String)>)> {
    use std::path::{Component, Path};

    let src = Path::new(src_root);
    let dst = Path::new(dst_root);
    let tgt = Path::new(target);

    if let Ok(rel) = tgt.strip_prefix(src) {
        if rel.as_os_str().is_empty() {
            return Some((dst_root.to_string(), None));
        }
        // Preserve `dst_root`'s own separator style so POSIX paths stay `/` on Windows and
        // native Windows `C:\...` roots stay `\` without mixing separators.
        let sep = if dst_root.contains('\\') && !dst_root.contains('/') {
            '\\'
        } else if dst_root.contains('/') {
            '/'
        } else {
            std::path::MAIN_SEPARATOR
        };
        let mut out = dst_root.to_string();
        for comp in rel.components() {
            if let Component::Normal(seg) = comp {
                if !out.ends_with('/') && !out.ends_with('\\') {
                    out.push(sep);
                }
                out.push_str(&seg.to_string_lossy());
            }
        }
        return Some((out, None));
    }

    if let Ok(up_rel) = src.strip_prefix(tgt) {
        let depth = up_rel.components().count();
        if depth > 0 {
            let mut cur = dst;
            for _ in 0..depth {
                cur = cur.parent()?;
            }
            let new_dst = cur.to_string_lossy().into_owned();
            return Some((new_dst.clone(), Some((target.to_string(), new_dst))));
        }
    }

    None
}

impl Session {
    /// A fresh two-browser session opened at `left` and `right`.
    pub fn new(left: &str, right: &str) -> Self {
        let mut s = Session {
            name: None,
            layout: Layout::Standard,
            browsers: vec![],
            active_browser: 0,
            pairs: vec![],
            closed_pairs: vec![],
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
                show_hidden: None,
                columns: Default::default(),
                view: Default::default(),
            });
        }
        s
    }

    fn make_tab(&mut self, path: &str) -> Tab {
        let id = self.allocate_id();
        Tab {
            id,
            path: path.to_string(),
            alias: None,
            keep_alias: false,
            history: History::default(),
            last_used: now_ms(),
        }
    }

    fn used_ids(&self) -> std::collections::HashSet<u64> {
        let mut used_ids = std::collections::HashSet::new();
        for browser in &self.browsers {
            used_ids.extend(browser.tabs.iter().map(|tab| tab.id));
            used_ids.extend(browser.closed.iter().map(|tab| tab.id));
        }
        used_ids.extend(self.pairs.iter().map(|pair| pair.id));
        used_ids.extend(self.closed_pairs.iter().map(|pair| pair.id));
        used_ids
    }

    fn allocate_id(&mut self) -> u64 {
        let used_ids = self.used_ids();
        allocate_unique_id(&mut self.next_id, &used_ids)
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
                show_hidden: None,
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
            .chain(
                self.pairs
                    .iter()
                    .chain(self.closed_pairs.iter())
                    .map(|p| p.id),
            )
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(max_id.checked_add(1).unwrap_or(1)).max(1);
        for i in 0..self.browsers.len() {
            if self.browsers[i].tabs.is_empty() {
                let t = self.make_tab(fallback);
                self.browsers[i].tabs.push(t);
            }
            let b = &mut self.browsers[i];
            b.active = b.active.min(b.tabs.len() - 1);
        }
        self.active_browser = self.active_browser.min(1);
        self.sanitize_pairs();
    }

    fn sanitize_pairs(&mut self) {
        let left_paths: std::collections::HashMap<u64, String> = self.browsers[0]
            .tabs
            .iter()
            .map(|t| (t.id, t.path.clone()))
            .collect();
        let right_paths: std::collections::HashMap<u64, String> = self.browsers[1]
            .tabs
            .iter()
            .map(|t| (t.id, t.path.clone()))
            .collect();
        let used_tab_ids: std::collections::HashSet<u64> = self
            .browsers
            .iter()
            .flat_map(|browser| browser.tabs.iter().chain(browser.closed.iter()))
            .map(|tab| tab.id)
            .collect();
        let mut used_ids = self.used_ids();
        let mut used_left = std::collections::HashSet::new();
        let mut used_right = std::collections::HashSet::new();
        let mut seen_pair_ids = std::collections::HashSet::new();
        let mut next_id = self.next_id;
        self.pairs.retain_mut(|p| {
            let Some(lp) = left_paths.get(&p.left_tab_id) else {
                return false;
            };
            let Some(rp) = right_paths.get(&p.right_tab_id) else {
                return false;
            };
            if !used_left.insert(p.left_tab_id) || !used_right.insert(p.right_tab_id) {
                return false;
            }
            if p.id == 0 || used_tab_ids.contains(&p.id) || !seen_pair_ids.insert(p.id) {
                p.id = allocate_unique_id(&mut next_id, &used_ids);
                seen_pair_ids.insert(p.id);
                used_ids.insert(p.id);
            }
            if p.left_root.is_empty() {
                p.left_root = lp.clone();
            }
            if p.right_root.is_empty() {
                p.right_root = rp.clone();
            }
            p.color_slot %= PAIR_COLOR_SLOTS;
            true
        });
        self.next_id = next_id;
        if self.closed_pairs.len() > MAX_CLOSED {
            let drain = self.closed_pairs.len() - MAX_CLOSED;
            self.closed_pairs.drain(0..drain);
        }
    }

    fn find_pair_index_for_tab(&self, b: usize, tab_id: u64) -> Option<usize> {
        let b = b.min(1);
        self.pairs.iter().position(|p| {
            if b == 0 {
                p.left_tab_id == tab_id
            } else {
                p.right_tab_id == tab_id
            }
        })
    }

    fn tab_index_by_id(&self, b: usize, tab_id: u64) -> Option<usize> {
        self.browser(b).tabs.iter().position(|t| t.id == tab_id)
    }

    fn navigate_tab_by_id(&mut self, b: usize, tab_id: u64, path: &str) -> bool {
        let Some(idx) = self.tab_index_by_id(b, tab_id) else {
            return false;
        };
        let t = &mut self.bm(b).tabs[idx];
        if t.path == path {
            return false;
        }
        let from = std::mem::replace(&mut t.path, path.to_string());
        t.history.visit(&from);
        if !t.keep_alias {
            t.alias = None;
        }
        t.last_used = now_ms();
        true
    }

    /// Mirrors `target` onto the partner tab if browser `b`'s active tab is in a `Tandem` pair.
    /// Returns the partner's new path if mirrored.
    fn mirror_tandem(&mut self, b: usize, target: &str) -> Option<String> {
        let b = b.min(1);
        let tab_id = self.active_tab(b).id;
        let pair_idx = self.find_pair_index_for_tab(b, tab_id)?;
        let pair = &self.pairs[pair_idx];
        if pair.mode != PairMode::Tandem {
            return None;
        }
        let (src_root, dst_root, partner_id) = if b == 0 {
            (
                pair.left_root.clone(),
                pair.right_root.clone(),
                pair.right_tab_id,
            )
        } else {
            (
                pair.right_root.clone(),
                pair.left_root.clone(),
                pair.left_tab_id,
            )
        };
        let (partner_path, reanchor) = tandem_mirror_path(&src_root, &dst_root, target)?;
        if let Some((new_src, new_dst)) = reanchor {
            if b == 0 {
                self.pairs[pair_idx].left_root = new_src;
                self.pairs[pair_idx].right_root = new_dst;
            } else {
                self.pairs[pair_idx].right_root = new_src;
                self.pairs[pair_idx].left_root = new_dst;
            }
        }
        let ob = 1 - b;
        if let Some(pidx) = self.tab_index_by_id(ob, partner_id) {
            self.bm(ob).active = pidx;
        }
        self.navigate_tab_by_id(ob, partner_id, &partner_path);
        Some(partner_path)
    }

    fn detach_pairs_for_tab(&mut self, b: usize, tab_id: u64, remember: bool) {
        let b = b.min(1);
        let mut i = 0;
        while i < self.pairs.len() {
            let matches = if b == 0 {
                self.pairs[i].left_tab_id == tab_id
            } else {
                self.pairs[i].right_tab_id == tab_id
            };
            if matches {
                let removed = self.pairs.remove(i);
                if remember {
                    self.closed_pairs.push(removed);
                    if self.closed_pairs.len() > MAX_CLOSED {
                        self.closed_pairs.remove(0);
                    }
                }
            } else {
                i += 1;
            }
        }
    }

    fn next_pair_color_slot(&self) -> u8 {
        for slot in 0..PAIR_COLOR_SLOTS {
            if !self.pairs.iter().any(|p| p.color_slot == slot) {
                return slot;
            }
        }
        (self.pairs.len() as u8) % PAIR_COLOR_SLOTS
    }

    /// Pairs a left tab (`left_idx` in browser 0) with a right tab (`right_idx` in browser 1).
    pub fn pair_tabs(
        &mut self,
        left_idx: usize,
        right_idx: usize,
        mode: PairMode,
        flow_dir: Option<FlowDirection>,
        flow_op: Option<FlowOp>,
    ) -> Option<u64> {
        let left = self.browser(0).tabs.get(left_idx)?.clone();
        let right = self.browser(1).tabs.get(right_idx)?.clone();
        // If these exact two tabs are already paired, update their mode in place.
        if let Some(existing) = self
            .pairs
            .iter_mut()
            .find(|p| p.left_tab_id == left.id && p.right_tab_id == right.id)
        {
            if mode == PairMode::Tandem && existing.mode != PairMode::Tandem {
                existing.left_root = left.path;
                existing.right_root = right.path;
            }
            existing.mode = mode;
            if let Some(d) = flow_dir {
                existing.flow_dir = d;
            }
            if let Some(o) = flow_op {
                existing.flow_op = o;
            }
            let id = existing.id;
            return Some(id);
        }
        let id = self.allocate_id();
        self.detach_pairs_for_tab(0, left.id, false);
        self.detach_pairs_for_tab(1, right.id, false);
        let color_slot = self.next_pair_color_slot();
        self.pairs.push(TabPair {
            id,
            left_tab_id: left.id,
            right_tab_id: right.id,
            mode,
            left_root: left.path,
            right_root: right.path,
            flow_dir: flow_dir.unwrap_or_default(),
            flow_op: flow_op.unwrap_or_default(),
            color_slot,
        });
        // Activate both tabs together so the newly paired bridge is immediately live.
        self.bm(0).active = left_idx;
        self.bm(1).active = right_idx;
        Some(id)
    }

    pub fn unpair(&mut self, pair_id: u64) -> bool {
        let before = self.pairs.len();
        self.pairs.retain(|p| p.id != pair_id);
        self.pairs.len() != before
    }

    pub fn set_pair_mode(
        &mut self,
        pair_id: u64,
        mode: PairMode,
        flow_dir: Option<FlowDirection>,
        flow_op: Option<FlowOp>,
    ) -> bool {
        let Some(idx) = self.pairs.iter().position(|p| p.id == pair_id) else {
            return false;
        };
        let entering_tandem = mode == PairMode::Tandem && self.pairs[idx].mode != PairMode::Tandem;
        let left_id = self.pairs[idx].left_tab_id;
        let right_id = self.pairs[idx].right_tab_id;
        if entering_tandem {
            let lp = self
                .tab_index_by_id(0, left_id)
                .map(|i| self.browser(0).tabs[i].path.clone());
            let rp = self
                .tab_index_by_id(1, right_id)
                .map(|i| self.browser(1).tabs[i].path.clone());
            if let (Some(lp), Some(rp)) = (lp, rp) {
                self.pairs[idx].left_root = lp;
                self.pairs[idx].right_root = rp;
            }
        }
        self.pairs[idx].mode = mode;
        if let Some(d) = flow_dir {
            self.pairs[idx].flow_dir = d;
        }
        if let Some(o) = flow_op {
            self.pairs[idx].flow_op = o;
        }
        true
    }

    /// Re-anchors a pair's Tandem roots to the two tabs' current paths.
    pub fn reanchor_pair(&mut self, pair_id: u64) -> bool {
        let Some(idx) = self.pairs.iter().position(|p| p.id == pair_id) else {
            return false;
        };
        let left_id = self.pairs[idx].left_tab_id;
        let right_id = self.pairs[idx].right_tab_id;
        let (Some(li), Some(ri)) = (
            self.tab_index_by_id(0, left_id),
            self.tab_index_by_id(1, right_id),
        ) else {
            return false;
        };
        self.pairs[idx].left_root = self.browser(0).tabs[li].path.clone();
        self.pairs[idx].right_root = self.browser(1).tabs[ri].path.clone();
        true
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
    /// If the tab is in a `Tandem` pair, mirrors the relative navigation onto its partner.
    pub fn navigate(&mut self, b: usize, path: &str) {
        if !self.navigate_solo(b, path) {
            return;
        }
        self.mirror_tandem(b, path);
    }

    /// Navigates only browser `b`'s active tab without triggering Tandem mirroring
    /// (used for non-destructive missing-folder recovery such as "Open parent").
    pub fn navigate_solo(&mut self, b: usize, path: &str) -> bool {
        let t = self.active_tab_mut(b);
        if t.path == path {
            return false;
        }
        let from = std::mem::replace(&mut t.path, path.to_string());
        t.history.visit(&from);
        if !t.keep_alias {
            t.alias = None;
        }
        t.last_used = now_ms();
        true
    }

    pub fn back(&mut self, b: usize) -> bool {
        let t = self.active_tab_mut(b);
        match t.history.back(&t.path.clone()) {
            Some(p) => {
                t.path = p.clone();
                t.last_used = now_ms();
                self.mirror_tandem(b, &p);
                true
            }
            None => false,
        }
    }

    pub fn forward(&mut self, b: usize) -> bool {
        let t = self.active_tab_mut(b);
        match t.history.forward(&t.path.clone()) {
            Some(p) => {
                t.path = p.clone();
                t.last_used = now_ms();
                self.mirror_tandem(b, &p);
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
        let b = b.min(1);
        let mut activated_id = None;
        {
            let br = self.bm(b);
            if idx < br.tabs.len() {
                br.active = idx;
                br.tabs[idx].last_used = now_ms();
                activated_id = Some(br.tabs[idx].id);
            }
        }
        self.active_browser = b;
        // Activating a paired tab brings its partner tab forward on the opposite side.
        if let Some(tab_id) = activated_id {
            if let Some(pair_idx) = self.find_pair_index_for_tab(b, tab_id) {
                let partner_id = if b == 0 {
                    self.pairs[pair_idx].right_tab_id
                } else {
                    self.pairs[pair_idx].left_tab_id
                };
                let ob = 1 - b;
                if let Some(pidx) = self.tab_index_by_id(ob, partner_id) {
                    let obr = self.bm(ob);
                    obr.active = pidx;
                    obr.tabs[pidx].last_used = now_ms();
                }
            }
        }
    }

    /// Closes a tab. The last tab of a browser is never removed; it would leave
    /// the browser empty, so it is kept (the UI may collapse the browser instead).
    pub fn close_tab(&mut self, b: usize, idx: usize) -> bool {
        let b = b.min(1);
        let removed_id = {
            let br = self.bm(b);
            if br.tabs.len() <= 1 || idx >= br.tabs.len() {
                return false;
            }
            let t = br.tabs.remove(idx);
            let id = t.id;
            br.closed.push(t);
            if br.closed.len() > MAX_CLOSED {
                br.closed.remove(0);
            }
            if br.active > idx || br.active >= br.tabs.len() {
                br.active = br.active.saturating_sub(1);
            }
            id
        };
        self.detach_pairs_for_tab(b, removed_id, true);
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
        let b = b.min(1);
        let reopened_id = {
            let br = self.bm(b);
            match br.closed.pop() {
                Some(mut t) => {
                    let id = t.id;
                    t.last_used = now_ms();
                    br.tabs.push(t);
                    br.active = br.tabs.len() - 1;
                    id
                }
                None => return false,
            }
        };
        // Restore a recently detached pair if its partner is still open and unpaired.
        let ob = 1 - b;
        if let Some(pos) = self.closed_pairs.iter().rposition(|p| {
            let (mine, partner) = if b == 0 {
                (p.left_tab_id, p.right_tab_id)
            } else {
                (p.right_tab_id, p.left_tab_id)
            };
            mine == reopened_id
                && self.tab_index_by_id(ob, partner).is_some()
                && self.find_pair_index_for_tab(ob, partner).is_none()
        }) {
            let restored = self.closed_pairs.remove(pos);
            let partner_id = if b == 0 {
                restored.right_tab_id
            } else {
                restored.left_tab_id
            };
            self.pairs.push(restored);
            if let Some(pidx) = self.tab_index_by_id(ob, partner_id) {
                self.bm(ob).active = pidx;
            }
        }
        true
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
        let from_b = from_b.min(1);
        let to_b = 1 - from_b;
        let Some(src) = self.browser(from_b).tabs.get(idx).cloned() else {
            return;
        };
        let allocated_id = if copy || self.browser(from_b).tabs.len() == 1 {
            Some(self.allocate_id())
        } else {
            None
        };
        let src_id = src.id;
        let mut t = src;
        if copy {
            t.id = allocated_id.expect("copy allocates a fresh tab ID");
        } else {
            self.detach_pairs_for_tab(from_b, src_id, false);
            if !self.close_tab(from_b, idx) {
                // Moving the only tab: leave a clone behind so the browser is not empty.
                t.id = allocated_id.expect("moving the only tab allocates a replacement ID");
            }
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
        let left_id = self.active_tab(0).id;
        let right_id = self.active_tab(1).id;
        let same_pair = self
            .pairs
            .iter()
            .position(|p| p.left_tab_id == left_id && p.right_tab_id == right_id);

        let (l, r) = self.browsers.split_at_mut(1);
        let (a, b) = (l[0].active, r[0].active);
        std::mem::swap(&mut l[0].tabs[a], &mut r[0].tabs[b]);
        let now = now_ms();
        l[0].tabs[a].last_used = now;
        r[0].tabs[b].last_used = now;

        if let Some(pidx) = same_pair {
            let p = &mut self.pairs[pidx];
            std::mem::swap(&mut p.left_tab_id, &mut p.right_tab_id);
            std::mem::swap(&mut p.left_root, &mut p.right_root);
            p.flow_dir = match p.flow_dir {
                FlowDirection::LeftToRight => FlowDirection::RightToLeft,
                FlowDirection::RightToLeft => FlowDirection::LeftToRight,
            };
        } else {
            // Avoid ending up with a pair whose ends sit on the same side.
            self.detach_pairs_for_tab(0, left_id, false);
            self.detach_pairs_for_tab(1, right_id, false);
        }
    }

    pub fn set_alias(&mut self, b: usize, idx: usize, alias: Option<String>, keep: bool) {
        if let Some(t) = self.bm(b).tabs.get_mut(idx) {
            t.alias = alias.filter(|a| !a.trim().is_empty());
            t.keep_alias = keep && t.alias.is_some();
        }
    }
}

/// Maximum number of workspaces kept in one window store.
pub const MAX_WORKSPACES: usize = 9;

/// One workspace context: an optional name and accent tint plus a full [`Session`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub accent: Option<String>,
    pub session: Session,
}

/// Lightweight metadata for a workspace pill and Drop Portal target popover.
/// Keeps inactive workspaces lazy: no directory listings or watchers are loaded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSummary {
    pub id: u64,
    pub index: usize,
    pub name: Option<String>,
    pub accent: Option<String>,
    pub layout: Layout,
    pub active_browser: usize,
    pub left_path: String,
    pub right_path: String,
    pub left_tabs: usize,
    pub right_tabs: usize,
    pub pair_count: usize,
}

/// Snapshot returned to the UI: summaries for every workspace plus the active [`Session`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    pub workspaces: Vec<WorkspaceSummary>,
    pub active: usize,
    pub session: Session,
}

/// Persistent collection of numbered workspaces (`1 2 3 …`) owned by a window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceStore {
    #[serde(default = "workspace_store_version")]
    pub version: u32,
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    next_id: u64,
}

fn workspace_store_version() -> u32 {
    2
}

/// On-disk format of `session.json`: either the workspace store (v2) or a legacy
/// single-session file from an earlier version of Gall, migrated transparently.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum StoredSession {
    Workspaces(WorkspaceStore),
    Legacy(Session),
}

impl StoredSession {
    /// Converts a loaded file (v2 workspace store or legacy single session) into a
    /// sanitized [`WorkspaceStore`].
    pub fn into_store(self, fallback: &str) -> WorkspaceStore {
        let mut store = match self {
            StoredSession::Workspaces(ws) => ws,
            StoredSession::Legacy(session) => WorkspaceStore::from_session(session),
        };
        store.sanitize(fallback);
        store
    }
}

fn normalize_name(name: Option<String>) -> Option<String> {
    name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn normalize_accent(accent: Option<String>) -> Option<String> {
    let raw = accent?.trim().to_string();
    if raw.len() == 7 && raw.starts_with('#') && raw.bytes().skip(1).all(|b| b.is_ascii_hexdigit())
    {
        Some(raw.to_ascii_lowercase())
    } else {
        None
    }
}

impl WorkspaceStore {
    /// Wraps an existing single [`Session`] as Workspace 1 (used both for fresh
    /// windows and for automatic migration from legacy `session.json`).
    pub fn from_session(session: Session) -> Self {
        let name = normalize_name(session.name.clone());
        WorkspaceStore {
            version: workspace_store_version(),
            workspaces: vec![Workspace {
                id: 1,
                name,
                accent: None,
                session,
            }],
            active: 0,
            next_id: 2,
        }
    }

    /// Repairs invariants after deserialization from disk.
    pub fn sanitize(&mut self, fallback: &str) {
        self.version = workspace_store_version();
        if self.workspaces.is_empty() {
            self.workspaces.push(Workspace {
                id: 1,
                name: None,
                accent: None,
                session: Session::new(fallback, fallback),
            });
        }
        if self.workspaces.len() > MAX_WORKSPACES {
            self.workspaces.truncate(MAX_WORKSPACES);
        }
        let mut seen = std::collections::HashSet::new();
        let max_id = self
            .workspaces
            .iter()
            .map(|w| w.id)
            .max()
            .unwrap_or(0)
            .max(self.next_id.saturating_sub(1));
        let mut next_id = self.next_id.max(max_id.checked_add(1).unwrap_or(1)).max(1);
        let mut used_ids: std::collections::HashSet<u64> = self
            .workspaces
            .iter()
            .map(|workspace| workspace.id)
            .collect();
        for ws in &mut self.workspaces {
            if ws.id == 0 || !seen.insert(ws.id) {
                ws.id = allocate_unique_id(&mut next_id, &used_ids);
                seen.insert(ws.id);
                used_ids.insert(ws.id);
            }
            ws.name = normalize_name(ws.name.take());
            ws.accent = normalize_accent(ws.accent.take());
            ws.session.sanitize(fallback);
        }
        self.next_id = next_id;
        self.active = self.active.min(self.workspaces.len() - 1);
    }

    pub fn active_workspace(&self) -> &Workspace {
        &self.workspaces[self.active.min(self.workspaces.len() - 1)]
    }

    pub fn active_workspace_mut(&mut self) -> &mut Workspace {
        let idx = self.active.min(self.workspaces.len() - 1);
        &mut self.workspaces[idx]
    }

    pub fn active_session(&self) -> &Session {
        &self.active_workspace().session
    }

    pub fn active_session_mut(&mut self) -> &mut Session {
        &mut self.active_workspace_mut().session
    }

    pub fn session_mut_for_id(&mut self, workspace_id: u64) -> Option<&mut Session> {
        self.workspaces
            .iter_mut()
            .find(|workspace| workspace.id == workspace_id)
            .map(|workspace| &mut workspace.session)
    }

    pub fn summaries(&self) -> Vec<WorkspaceSummary> {
        self.workspaces
            .iter()
            .enumerate()
            .map(|(index, ws)| WorkspaceSummary {
                id: ws.id,
                index,
                name: ws.name.clone(),
                accent: ws.accent.clone(),
                layout: ws.session.layout,
                active_browser: ws.session.active_browser,
                left_path: ws.session.active_tab(0).path.clone(),
                right_path: ws.session.active_tab(1).path.clone(),
                left_tabs: ws.session.browser(0).tabs.len(),
                right_tabs: ws.session.browser(1).tabs.len(),
                pair_count: ws.session.pairs.len(),
            })
            .collect()
    }

    pub fn snapshot(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            workspaces: self.summaries(),
            active: self.active,
            session: self.active_session().clone(),
        }
    }

    pub fn switch(&mut self, idx: usize) -> bool {
        if idx >= self.workspaces.len() {
            return false;
        }
        self.active = idx;
        true
    }

    pub fn create(
        &mut self,
        left: &str,
        right: &str,
        name: Option<String>,
        accent: Option<String>,
        activate: bool,
    ) -> Option<u64> {
        if self.workspaces.len() >= MAX_WORKSPACES {
            return None;
        }
        let used_ids: std::collections::HashSet<u64> = self
            .workspaces
            .iter()
            .map(|workspace| workspace.id)
            .collect();
        let id = allocate_unique_id(&mut self.next_id, &used_ids);
        let session = Session::new(left, right);
        self.workspaces.push(Workspace {
            id,
            name: normalize_name(name),
            accent: normalize_accent(accent),
            session,
        });
        if activate {
            self.active = self.workspaces.len() - 1;
        }
        Some(id)
    }

    pub fn duplicate(&mut self, idx: usize, activate: bool) -> Option<u64> {
        if self.workspaces.len() >= MAX_WORKSPACES {
            return None;
        }
        let src = self.workspaces.get(idx)?.clone();
        let used_ids: std::collections::HashSet<u64> = self
            .workspaces
            .iter()
            .map(|workspace| workspace.id)
            .collect();
        let id = allocate_unique_id(&mut self.next_id, &used_ids);
        let name = src.name.map(|n| format!("{n} copy"));
        let insert_at = (idx + 1).min(self.workspaces.len());
        self.workspaces.insert(
            insert_at,
            Workspace {
                id,
                name,
                accent: src.accent,
                session: src.session,
            },
        );
        if activate {
            self.active = insert_at;
        } else if self.active >= insert_at {
            self.active += 1;
        }
        Some(id)
    }

    pub fn rename(&mut self, idx: usize, name: Option<String>) -> bool {
        let Some(ws) = self.workspaces.get_mut(idx) else {
            return false;
        };
        ws.name = normalize_name(name);
        true
    }

    pub fn set_accent(&mut self, idx: usize, accent: Option<String>) -> bool {
        let Some(ws) = self.workspaces.get_mut(idx) else {
            return false;
        };
        ws.accent = normalize_accent(accent);
        true
    }

    pub fn reorder(&mut self, from: usize, to: usize) -> bool {
        if from >= self.workspaces.len() || to >= self.workspaces.len() || from == to {
            return false;
        }
        let active_id = self.active_workspace().id;
        let ws = self.workspaces.remove(from);
        let dest = to.min(self.workspaces.len());
        self.workspaces.insert(dest, ws);
        self.active = self
            .workspaces
            .iter()
            .position(|w| w.id == active_id)
            .unwrap_or(0);
        true
    }

    /// Closes workspace `idx`. Never removes the last workspace.
    pub fn close(&mut self, idx: usize) -> bool {
        if self.workspaces.len() <= 1 || idx >= self.workspaces.len() {
            return false;
        }
        self.workspaces.remove(idx);
        if self.active > idx || self.active >= self.workspaces.len() {
            self.active = self.active.saturating_sub(1);
        }
        true
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

    #[test]
    fn show_hidden_persists_per_browser_and_defaults_to_none() {
        let mut s = Session::new("/a", "/z");
        assert_eq!(s.browsers[0].show_hidden, None);
        assert_eq!(s.browsers[1].show_hidden, None);

        // Old sessions saved before per-browser showHidden load as None (synced).
        let mut v = serde_json::to_value(&s).unwrap();
        for b in v["browsers"].as_array_mut().unwrap() {
            b.as_object_mut().unwrap().remove("showHidden");
        }
        let loaded: Session = serde_json::from_value(v).unwrap();
        assert_eq!(loaded.browsers[0].show_hidden, None);
        assert_eq!(loaded.browsers[1].show_hidden, None);

        // Per-browser override round-trips and stays on its side when folders swap.
        s.browsers[0].show_hidden = Some(true);
        s.browsers[1].show_hidden = Some(false);
        let round: Session = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(round.browsers[0].show_hidden, Some(true));
        assert_eq!(round.browsers[1].show_hidden, Some(false));

        s.swap_active_tabs();
        assert_eq!(s.browsers[0].show_hidden, Some(true));
        assert_eq!(s.browsers[1].show_hidden, Some(false));
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
        s.browsers[0].tabs[0].id = u64::MAX;
        s.next_id = u64::MAX;
        s.browsers[0].active = 9;
        s.sanitize("/home");
        assert_eq!(s.browsers.len(), 2);
        assert_eq!(s.browsers[0].active, 0);
        assert_ne!(s.browsers[0].tabs[0].id, s.browsers[1].tabs[0].id);
    }

    #[test]
    fn sanitize_repairs_pair_and_tab_id_collision_at_u64_max() {
        let mut session = Session::new("/left", "/right");
        session.browsers[0].tabs[0].id = u64::MAX;
        session.next_id = u64::MAX;
        session.pairs.push(TabPair {
            id: u64::MAX,
            left_tab_id: u64::MAX,
            right_tab_id: session.browsers[1].tabs[0].id,
            mode: PairMode::Linked,
            left_root: "/left".to_string(),
            right_root: "/right".to_string(),
            flow_dir: FlowDirection::default(),
            flow_op: FlowOp::default(),
            color_slot: 0,
        });

        session.sanitize("/home");
        assert_eq!(session.pairs.len(), 1);
        assert_ne!(session.pairs[0].id, u64::MAX);
        assert_ne!(session.pairs[0].id, session.browsers[0].tabs[0].id);

        let new_tab_id = session.new_tab(0, Some("/new"), NewTabPlacement::End, true);
        let ids = session.used_ids();
        assert!(ids.contains(&new_tab_id));
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn legacy_session_migrates_to_workspace_one() {
        let mut legacy = Session::new("/projects", "/notes");
        legacy.name = Some("Main".into());
        legacy.navigate(0, "/projects/gall");
        legacy.layout = Layout::Columns;

        let raw = serde_json::to_string(&legacy).unwrap();
        let stored: StoredSession = serde_json::from_str(&raw).unwrap();
        assert!(matches!(stored, StoredSession::Legacy(_)));
        let ws = stored.into_store("/home");

        assert_eq!(ws.version, 2);
        assert_eq!(ws.workspaces.len(), 1);
        assert_eq!(ws.active, 0);
        assert_eq!(ws.workspaces[0].id, 1);
        assert_eq!(ws.workspaces[0].name.as_deref(), Some("Main"));
        assert_eq!(ws.active_session().active_tab(0).path, "/projects/gall");
        assert_eq!(ws.active_session().active_tab(1).path, "/notes");
        assert_eq!(ws.active_session().layout, Layout::Columns);

        // Re-serialized as v2 WorkspaceStore and parsed back as Workspaces variant.
        let v2_raw = serde_json::to_string(&ws).unwrap();
        let stored_v2: StoredSession = serde_json::from_str(&v2_raw).unwrap();
        assert!(matches!(stored_v2, StoredSession::Workspaces(_)));
        assert_eq!(stored_v2.into_store("/home"), ws);
    }

    #[test]
    fn workspaces_lifecycle_reorder_and_summaries() {
        let mut store = WorkspaceStore::from_session(Session::new("/a", "/b"));
        let id2 = store
            .create(
                "/src",
                "/dist",
                Some("  Build  ".into()),
                Some("#3D8BFD".into()),
                true,
            )
            .unwrap();
        assert_eq!(store.active, 1);
        assert_eq!(store.active_workspace().id, id2);
        assert_eq!(store.active_workspace().name.as_deref(), Some("Build"));
        assert_eq!(store.active_workspace().accent.as_deref(), Some("#3d8bfd"));

        // Invalid accent is rejected cleanly.
        assert!(store.set_accent(1, Some("not-a-color".into())));
        assert_eq!(store.workspaces[1].accent, None);
        assert!(store.set_accent(1, Some("#3fb27f".into())));
        assert_eq!(store.workspaces[1].accent.as_deref(), Some("#3fb27f"));

        // Reorder preserves the active workspace by ID.
        assert!(store.reorder(1, 0));
        assert_eq!(store.active, 0);
        assert_eq!(store.active_workspace().id, id2);

        // Summaries expose both panes lazily for Drop Portal.
        let sums = store.summaries();
        assert_eq!(sums.len(), 2);
        assert_eq!(sums[0].left_path, "/src");
        assert_eq!(sums[0].right_path, "/dist");
        assert_eq!(sums[1].left_path, "/a");
        assert_eq!(sums[1].right_path, "/b");

        // Duplicate clones the full session into a new workspace.
        let dup_id = store.duplicate(0, false).unwrap();
        assert_ne!(dup_id, id2);
        assert_eq!(store.workspaces.len(), 3);
        assert_eq!(store.workspaces[1].name.as_deref(), Some("Build copy"));
        assert_eq!(store.active, 0);

        // Closing adjusts active index and never removes the final workspace.
        assert!(store.close(0));
        assert_eq!(store.workspaces.len(), 2);
        assert!(store.close(1));
        assert_eq!(store.workspaces.len(), 1);
        assert!(!store.close(0));
    }

    #[test]
    fn workspace_id_repair_and_allocation_handle_u64_max() {
        let mut store = WorkspaceStore::from_session(Session::new("/a", "/b"));
        store.workspaces[0].id = u64::MAX;
        let duplicate = store.workspaces[0].clone();
        store.workspaces.push(duplicate);
        store.next_id = u64::MAX;

        store.sanitize("/home");
        let ids: std::collections::HashSet<_> = store
            .workspaces
            .iter()
            .map(|workspace| workspace.id)
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(!ids.contains(&0));

        let new_id = store.create("/c", "/d", None, None, false).unwrap();
        assert!(!ids.contains(&new_id));
    }

    #[test]
    fn workspace_session_mut_for_id_targets_inactive_workspace() {
        let mut store = WorkspaceStore::from_session(Session::new("/first", "/first-right"));
        let first_id = store.workspaces[0].id;
        let second_id = store
            .create("/second", "/second-right", None, None, true)
            .unwrap();

        store
            .session_mut_for_id(first_id)
            .unwrap()
            .navigate(0, "/first/updated");

        assert_eq!(store.active_workspace().id, second_id);
        assert_eq!(store.active_session().active_tab(0).path, "/second");
        assert_eq!(
            store
                .session_mut_for_id(first_id)
                .unwrap()
                .active_tab(0)
                .path,
            "/first/updated"
        );
        assert!(store.session_mut_for_id(u64::MAX).is_none());
    }

    #[test]
    fn live_tab_pairs_linked_tandem_flow_and_reopen() {
        let mut s = Session::new("/repo-a", "/repo-b");
        s.new_tab(0, Some("/other-l"), NewTabPlacement::End, false);
        s.new_tab(1, Some("/other-r"), NewTabPlacement::End, false);

        // Pair left[0] (/repo-a) with right[0] (/repo-b) in Tandem mode.
        let pid = s
            .pair_tabs(0, 0, PairMode::Tandem, None, None)
            .expect("pair created");
        assert_eq!(s.pairs.len(), 1);

        // Activating an unpaired tab does not move the other side, then activating either paired
        // tab brings BOTH halves forward in one click.
        s.activate(0, 1);
        assert_eq!(s.active_tab(0).path, "/other-l");
        assert_eq!(s.active_tab(1).path, "/repo-b");
        s.activate(1, 1);
        assert_eq!(s.active_tab(1).path, "/other-r");
        s.activate(0, 0);
        assert_eq!(s.active_tab(0).path, "/repo-a");
        assert_eq!(s.active_tab(1).path, "/repo-b");

        // Tandem relative walk down, across siblings, and back/forward.
        s.navigate(0, "/repo-a/src/ui");
        assert_eq!(s.active_tab(1).path, "/repo-b/src/ui");
        // Verify separator style stays consistent for both POSIX roots and backslash roots.
        assert_eq!(
            tandem_mirror_path("/repo-a", "/repo-b", "/repo-a/src/ui"),
            Some(("/repo-b/src/ui".to_string(), None))
        );
        assert_eq!(
            tandem_mirror_path("/", "/mirror", "/src/ui"),
            Some(("/mirror/src/ui".to_string(), None))
        );
        s.navigate(0, "/repo-a/tests");
        assert_eq!(s.active_tab(1).path, "/repo-b/tests");
        assert!(s.back(0));
        assert_eq!(s.active_tab(0).path, "/repo-a/src/ui");
        assert_eq!(s.active_tab(1).path, "/repo-b/src/ui");

        // Navigating solo ("Open parent" recovery) leaves partner and pair untouched.
        assert!(s.navigate_solo(1, "/repo-b/src"));
        assert_eq!(s.active_tab(0).path, "/repo-a/src/ui");
        assert_eq!(s.active_tab(1).path, "/repo-b/src");
        assert_eq!(s.pairs.len(), 1);

        // Unrelated jump outside src_root keeps the pair intact without bogus partner jump.
        s.navigate(0, "/tmp/scratch");
        assert_eq!(s.active_tab(1).path, "/repo-b/src");
        assert_eq!(s.pairs.len(), 1);

        // Climbing above root re-anchors both sides cleanly.
        s.navigate(0, "/repo-a");
        assert_eq!(s.active_tab(1).path, "/repo-b");
        s.navigate(0, "/");
        assert_eq!(s.active_tab(1).path, "/");

        // Switch to Flow mode and swap keeps pair intact while reversing direction.
        assert!(s.set_pair_mode(
            pid,
            PairMode::Flow,
            Some(FlowDirection::LeftToRight),
            Some(FlowOp::Move),
        ));
        s.navigate_solo(0, "/left-flow");
        s.navigate_solo(1, "/right-flow");
        s.swap_active_tabs();
        assert_eq!(s.active_tab(0).path, "/right-flow");
        assert_eq!(s.active_tab(1).path, "/left-flow");
        assert_eq!(s.pairs.len(), 1);
        assert_eq!(s.pairs[0].flow_dir, FlowDirection::RightToLeft);

        // Closing one paired tab detaches; reopening restores the pair automatically!
        assert!(s.close_tab(0, 0));
        assert!(s.pairs.is_empty());
        assert!(s.reopen_closed(0));
        assert_eq!(s.pairs.len(), 1);
        assert_eq!(s.pairs[0].mode, PairMode::Flow);

        // Transferring a paired tab (move) detaches the pair so both ends never live on one side.
        let active_l = s.browser(0).active;
        s.transfer_tab(0, active_l, false);
        assert!(s.pairs.is_empty());
    }

    #[test]
    fn old_session_without_pairs_deserializes_cleanly() {
        let mut v = serde_json::to_value(Session::new("/a", "/b")).unwrap();
        v.as_object_mut().unwrap().remove("pairs");
        v.as_object_mut().unwrap().remove("closedPairs");
        let mut s: Session = serde_json::from_value(v).unwrap();
        s.sanitize("/home");
        assert!(s.pairs.is_empty());
    }
}
