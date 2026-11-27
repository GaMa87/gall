//! Shared search-filter evaluation and budgeted hidden-match counting.
//!
//! Quick Open, find in files and F3 all skip the same families of ignored items:
//! `.gallignore`, `CACHEDIR.TAG` cache directories, `.gitignore` / `.ignore` /
//! `.git/info/exclude` / global gitignore (when enabled), and configured exclude
//! globs (e.g. `node_modules`, `target`).
//!
//! This module serves two purposes without slowing down the normal visible search:
//! 1. **Background hidden-match counting** (`walk_hidden_matches`): a prioritized,
//!    strictly budgeted BFS walk that inspects `.gallignore` / `.gitignore` trees
//!    before massive excluded trees (`node_modules`, `target`, caches) and stops
//!    early on match cap, entry cap, wall-clock deadline, or cancellation.
//! 2. **Normal-filter classification** (`FilterMatcher::is_path_hidden`): used
//!    when `Alt+I` ("search everything") is active to mark only the results that
//!    normal filters would have hidden, caching parsed per-directory rules.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::overrides::{Override, OverrideBuilder};
use ignore::Match;
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// Normal search filters that `Alt+I` ("search everything") disables.
#[derive(Debug, Clone, Default)]
pub struct FilterConfig {
    /// Whether OS-hidden / dot-prefixed entries are included in the search.
    pub include_hidden: bool,
    /// Honour `.gitignore`, `.ignore`, `.git/info/exclude`, and global gitignore.
    pub respect_gitignore: bool,
    /// Global excluded folder/file globs (e.g. `node_modules`, `target`).
    pub exclude: Vec<String>,
    /// Honour `.gallignore` and `CACHEDIR.TAG` cache folders.
    pub use_gallignore: bool,
}

/// Number of hidden items that matched a query.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HiddenCount {
    pub count: usize,
    /// True when counting stopped early at the match cap or scan budget while
    /// at least one hidden match was found (display as `N+`).
    pub capped: bool,
}

/// Resource limits for background hidden-match counting.
#[derive(Debug, Clone, Copy)]
pub struct CountBudget {
    /// Stop once this many hidden matches have been found.
    pub max_matches: usize,
    /// Maximum directory entries inspected before giving up.
    pub max_entries: usize,
    /// Wall-clock deadline for the background probe.
    pub max_duration: Duration,
}

impl CountBudget {
    /// Default budget for file-name / fuzzy hidden-match counting (Quick Open, F3).
    pub fn name_default() -> Self {
        Self {
            max_matches: 100,
            max_entries: 25_000,
            max_duration: Duration::from_millis(200),
        }
    }

    /// Default budget for content search hidden-match counting (Find in Files).
    pub fn grep_default() -> Self {
        Self {
            max_matches: 100,
            max_entries: 15_000,
            max_duration: Duration::from_millis(350),
        }
    }
}

/// Why an entry is hidden by search filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterReason {
    /// Hidden by `.gallignore`, `.ignore`, `.gitignore`, `.git/info/exclude`, or global gitignore.
    IgnoreFile,
    /// Hidden by configured exclude globs (e.g. `node_modules`, `target`).
    Exclude,
    /// Hidden because the directory carries a valid `CACHEDIR.TAG`.
    Cache,
}

/// Immutable per-directory ignore rules linked to its parent directory.
#[derive(Debug)]
struct DirRules {
    gallignore: Option<Gitignore>,
    ignore_file: Option<Gitignore>,
    gitignore: Option<Gitignore>,
    /// Present when inside a Git repository: `(git_root, info/exclude matcher)`.
    git_repo: Option<Arc<GitRepoInfo>>,
    parent: Option<Arc<DirRules>>,
}

#[derive(Debug)]
struct GitRepoInfo {
    root: PathBuf,
    info_exclude: Option<Gitignore>,
}

/// Evaluates normal search filters for entries under a `root` directory,
/// caching per-directory ignore files and directory-level hidden status.
pub struct FilterMatcher {
    root: PathBuf,
    config: FilterConfig,
    overrides: Option<Override>,
    global_gitignore: Option<Gitignore>,
    root_rules: Arc<DirRules>,
    /// Cache from directory path -> `(rules, is_hidden_dir)` for directories under `root`.
    dir_cache: RwLock<HashMap<PathBuf, Arc<CachedDir>>>,
}

struct CachedDir {
    rules: Arc<DirRules>,
    /// True if this directory (or an ancestor below `root`) was itself hidden.
    hidden: bool,
}

impl FilterMatcher {
    pub fn new(root: &Path, config: FilterConfig) -> Self {
        let overrides = build_exclude_override(root, &config.exclude);
        let global_gitignore = if config.respect_gitignore {
            let (gi, _err) = Gitignore::global();
            (!gi.is_empty()).then_some(gi)
        } else {
            None
        };
        let root_rules = build_ancestor_rules(root, &config);
        let mut cache = HashMap::new();
        cache.insert(
            root.to_path_buf(),
            Arc::new(CachedDir {
                rules: root_rules.clone(),
                hidden: false,
            }),
        );
        Self {
            root: root.to_path_buf(),
            config,
            overrides,
            global_gitignore,
            root_rules,
            dir_cache: RwLock::new(cache),
        }
    }

    /// Returns whether `path` (which must be strictly inside `root`) would be hidden
    /// by normal search filters (either directly or via a hidden ancestor under `root`).
    pub fn is_path_hidden(&self, path: &Path, is_dir: bool) -> bool {
        if path == self.root {
            return false;
        }
        let Some(parent) = path.parent() else {
            return false;
        };
        let parent_state = self.ensure_dir(parent);
        if parent_state.hidden {
            return true;
        }
        self.check_entry(&parent_state.rules, path, is_dir)
            .is_some()
    }

    /// Evaluates whether a single entry directly inside a visible directory with
    /// `parent_rules` is hidden by normal search filters.
    fn check_entry(
        &self,
        parent_rules: &DirRules,
        path: &Path,
        is_dir: bool,
    ) -> Option<FilterReason> {
        // 1. Exclude overrides (e.g. `node_modules`, `target`) take highest precedence.
        if let Some(ov) = &self.overrides {
            match ov.matched(path, is_dir) {
                Match::Ignore(_) => return Some(FilterReason::Exclude),
                Match::Whitelist(_) => return None,
                Match::None => {}
            }
        }
        // 2. CACHEDIR.TAG directories (never root itself, which isn't passed here).
        if self.config.use_gallignore && is_dir && super::ignorefile::is_cache_dir(path) {
            return Some(FilterReason::Cache);
        }
        // 3. Per-directory ignore files from nearest directory upwards:
        //    `.gallignore` > `.ignore` > `.gitignore` (within git root).
        let git_root = parent_rules.git_repo.as_ref().map(|r| r.root.as_path());
        let mut cur = Some(parent_rules);
        while let Some(node) = cur {
            if let Some(gi) = &node.gallignore {
                match gi.matched(path, is_dir) {
                    Match::Ignore(_) => return Some(FilterReason::IgnoreFile),
                    Match::Whitelist(_) => return None,
                    Match::None => {}
                }
            }
            if let Some(gi) = &node.ignore_file {
                match gi.matched(path, is_dir) {
                    Match::Ignore(_) => return Some(FilterReason::IgnoreFile),
                    Match::Whitelist(_) => return None,
                    Match::None => {}
                }
            }
            if let (Some(gi), Some(gr)) = (&node.gitignore, git_root) {
                if gi.path().starts_with(gr) {
                    match gi.matched(path, is_dir) {
                        Match::Ignore(_) => return Some(FilterReason::IgnoreFile),
                        Match::Whitelist(_) => return None,
                        Match::None => {}
                    }
                }
            }
            cur = node.parent.as_deref();
        }
        // 4. Repository `.git/info/exclude` and global gitignore (only inside a git repo).
        if let Some(repo) = &parent_rules.git_repo {
            if let Some(gi) = &repo.info_exclude {
                match gi.matched(path, is_dir) {
                    Match::Ignore(_) => return Some(FilterReason::IgnoreFile),
                    Match::Whitelist(_) => return None,
                    Match::None => {}
                }
            }
            if let Some(gi) = &self.global_gitignore {
                match gi.matched(path, is_dir) {
                    Match::Ignore(_) => return Some(FilterReason::IgnoreFile),
                    Match::Whitelist(_) => return None,
                    Match::None => {}
                }
            }
        }
        None
    }

    fn ensure_dir(&self, dir: &Path) -> Arc<CachedDir> {
        if let Some(hit) = self.dir_cache.read().unwrap().get(dir) {
            return hit.clone();
        }
        // Walk up until we hit a cached ancestor (at worst `self.root`, or outside `root`).
        let mut missing = Vec::new();
        let mut cur = dir;
        let base = loop {
            if let Some(hit) = self.dir_cache.read().unwrap().get(cur) {
                break hit.clone();
            }
            if cur == self.root || !cur.starts_with(&self.root) {
                break Arc::new(CachedDir {
                    rules: self.root_rules.clone(),
                    hidden: false,
                });
            }
            missing.push(cur.to_path_buf());
            let Some(p) = cur.parent() else {
                break Arc::new(CachedDir {
                    rules: self.root_rules.clone(),
                    hidden: false,
                });
            };
            cur = p;
        };

        let mut state = base;
        for d in missing.into_iter().rev() {
            let hidden = state.hidden || self.check_entry(&state.rules, &d, true).is_some();
            let rules = if hidden {
                // Descendants of a hidden dir are always hidden; no need to load rules inside it.
                state.rules.clone()
            } else {
                load_child_rules(&d, &state.rules, &self.config)
            };
            let next = Arc::new(CachedDir { rules, hidden });
            self.dir_cache.write().unwrap().insert(d, next.clone());
            state = next;
        }
        state
    }
}

/// Builds an `Override` matcher that ignores every comma-separated glob in `exclude`.
pub fn build_exclude_override(root: &Path, exclude: &[String]) -> Option<Override> {
    let globs: Vec<&str> = exclude
        .iter()
        .flat_map(|g| g.split(','))
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .collect();
    if globs.is_empty() {
        return None;
    }
    let mut ob = OverrideBuilder::new(root);
    for g in globs {
        let _ = ob.add(&format!("!{g}"));
    }
    ob.build().ok()
}

fn load_gitignore_file(dir: &Path, file: &Path) -> Option<Gitignore> {
    if !file.is_file() {
        return None;
    }
    let mut b = GitignoreBuilder::new(dir);
    b.add(file);
    b.build().ok().filter(|g| !g.is_empty())
}

/// Resolves the `.git` metadata directory for a repository root (handling both
/// normal `.git/` directories and `.git` gitdir files used by worktrees/submodules).
fn resolve_git_dir(repo_root: &Path) -> Option<PathBuf> {
    let dot_git = repo_root.join(".git");
    let meta = std::fs::symlink_metadata(&dot_git).ok()?;
    if meta.is_dir() {
        return Some(dot_git);
    }
    if meta.is_file() {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let line = text.lines().next()?.trim();
        let rest = line.strip_prefix("gitdir:")?.trim();
        if rest.is_empty() {
            return None;
        }
        let p = Path::new(rest);
        return Some(if p.is_absolute() {
            p.to_path_buf()
        } else {
            repo_root.join(p)
        });
    }
    None
}

fn load_git_repo_info(repo_root: &Path) -> Option<Arc<GitRepoInfo>> {
    let git_dir = resolve_git_dir(repo_root)?;
    let exclude_file = git_dir.join("info").join("exclude");
    let info_exclude = load_gitignore_file(repo_root, &exclude_file);
    Some(Arc::new(GitRepoInfo {
        root: repo_root.to_path_buf(),
        info_exclude,
    }))
}

fn build_ancestor_rules(root: &Path, config: &FilterConfig) -> Arc<DirRules> {
    // Collect ancestors from outermost down to `root`.
    let mut chain: Vec<&Path> = root
        .ancestors()
        .filter(|d| !d.as_os_str().is_empty())
        .collect();
    chain.reverse();

    let mut parent: Option<Arc<DirRules>> = None;
    let mut git_repo: Option<Arc<GitRepoInfo>> = None;

    for dir in chain {
        if config.respect_gitignore {
            if let Some(info) = load_git_repo_info(dir) {
                git_repo = Some(info);
            }
        }
        let gallignore = if config.use_gallignore {
            load_gitignore_file(dir, &dir.join(super::ignorefile::GALL_IGNORE))
        } else {
            None
        };
        let ignore_file = if config.respect_gitignore {
            load_gitignore_file(dir, &dir.join(".ignore"))
        } else {
            None
        };
        let gitignore = if config.respect_gitignore && git_repo.is_some() {
            load_gitignore_file(dir, &dir.join(".gitignore"))
        } else {
            None
        };
        parent = Some(Arc::new(DirRules {
            gallignore,
            ignore_file,
            gitignore,
            git_repo: git_repo.clone(),
            parent,
        }));
    }

    parent.unwrap_or_else(|| {
        Arc::new(DirRules {
            gallignore: None,
            ignore_file: None,
            gitignore: None,
            git_repo: None,
            parent: None,
        })
    })
}

fn load_child_rules(dir: &Path, parent: &Arc<DirRules>, config: &FilterConfig) -> Arc<DirRules> {
    let git_repo = if config.respect_gitignore {
        load_git_repo_info(dir).or_else(|| parent.git_repo.clone())
    } else {
        None
    };
    let gallignore = if config.use_gallignore {
        load_gitignore_file(dir, &dir.join(super::ignorefile::GALL_IGNORE))
    } else {
        None
    };
    let ignore_file = if config.respect_gitignore {
        load_gitignore_file(dir, &dir.join(".ignore"))
    } else {
        None
    };
    let gitignore = if config.respect_gitignore && git_repo.is_some() {
        load_gitignore_file(dir, &dir.join(".gitignore"))
    } else {
        None
    };
    // Avoid allocating a new node when nothing changed in this directory.
    if gallignore.is_none()
        && ignore_file.is_none()
        && gitignore.is_none()
        && match (&git_repo, &parent.git_repo) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    {
        return parent.clone();
    }
    Arc::new(DirRules {
        gallignore,
        ignore_file,
        gitignore,
        git_repo,
        parent: Some(parent.clone()),
    })
}

/// Whether `name` / `entry` is OS-hidden (dot-prefixed, or Windows hidden attribute).
fn is_os_hidden(entry: &std::fs::DirEntry, name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        if entry
            .metadata()
            .is_ok_and(|m| super::entry::hidden_attr(&m))
        {
            return true;
        }
    }
    #[cfg(not(windows))]
    let _ = entry;
    false
}

/// Outcome of testing one hidden entry during [`walk_hidden_matches`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeMatch {
    /// Entry matched the query.
    Matched,
    /// Entry did not match.
    NoMatch,
    /// Caller's own work cap (e.g. max files opened for grep) was reached; stop probing.
    BudgetExhausted,
}

/// Prioritized, strictly budgeted BFS walk that counts entries hidden by
/// `config` that satisfy `on_hidden(path, name, is_dir)`.
///
/// Visible directories and `.gallignore`/`.gitignore`-hidden directories are
/// scanned before massive excluded trees (`node_modules`, `target`, `CACHEDIR.TAG`)
/// so a small `.gallignore`'d file next to `node_modules` is always found first.
///
/// `user_exclude` holds caller-supplied exclusions that apply even when `Alt+I`
/// is enabled (such as Find in Files' explicit exclude filter box).
pub fn walk_hidden_matches(
    root: &Path,
    config: &FilterConfig,
    user_exclude: Option<&Override>,
    budget: CountBudget,
    cancel: &AtomicBool,
    mut on_hidden: impl FnMut(&Path, &str, bool) -> ProbeMatch,
) -> HiddenCount {
    if budget.max_matches == 0 || budget.max_entries == 0 || cancel.load(Ordering::Relaxed) {
        return HiddenCount::default();
    }
    let started = Instant::now();
    let remote = super::index::remote_mounts();
    let mut state = WalkState {
        started,
        budget,
        matches: 0,
        visited: 0,
        budget_hit: false,
    };
    walk_one_root(
        root,
        false,
        config,
        user_exclude,
        &remote,
        &[],
        cancel,
        &mut state,
        &mut on_hidden,
    );
    finish_count(cancel, &state)
}

/// Counts hidden matches across multiple `roots` (for Quick Open "everywhere"),
/// probing `priority` folders (e.g. current folder, home) first when they lie
/// inside a root, skipping already-probed subtrees so no entry is counted twice,
/// and sharing one [`CountBudget`] across the whole probe.
pub fn walk_hidden_matches_multi(
    roots: &[PathBuf],
    priority: &[PathBuf],
    config: &FilterConfig,
    budget: CountBudget,
    cancel: &AtomicBool,
    mut on_hidden: impl FnMut(&Path, &str, bool) -> ProbeMatch,
) -> HiddenCount {
    if budget.max_matches == 0 || budget.max_entries == 0 || cancel.load(Ordering::Relaxed) {
        return HiddenCount::default();
    }
    let started = Instant::now();
    let remote = super::index::remote_mounts();
    let mut state = WalkState {
        started,
        budget,
        matches: 0,
        visited: 0,
        budget_hit: false,
    };

    // Build ordered (dir, containing_root) targets: priority folders inside any root
    // first, then remaining roots.
    let mut targets: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    for p in priority {
        if !p.is_dir() {
            continue;
        }
        let Some(containing) = roots.iter().find(|r| p.starts_with(r)) else {
            continue;
        };
        if targets.iter().any(|(d, _)| p.starts_with(d)) {
            continue;
        }
        targets.push((p.clone(), (p != containing).then(|| containing.clone())));
    }
    for r in roots {
        if !r.is_dir() || targets.iter().any(|(d, _)| r == d) {
            continue;
        }
        targets.push((r.clone(), None));
    }

    let mut scanned: Vec<PathBuf> = Vec::new();
    for (dir, containing_root) in targets {
        if cancel.load(Ordering::Relaxed) || state.budget_hit {
            break;
        }
        let initial_hidden = containing_root
            .as_deref()
            .is_some_and(|r| FilterMatcher::new(r, config.clone()).is_path_hidden(&dir, true));
        if initial_hidden {
            if let Some(name) = dir.file_name().map(|n| n.to_string_lossy()) {
                match on_hidden(&dir, &name, true) {
                    ProbeMatch::Matched => {
                        state.matches += 1;
                        if state.matches >= state.budget.max_matches {
                            state.budget_hit = true;
                            break;
                        }
                    }
                    ProbeMatch::NoMatch => {}
                    ProbeMatch::BudgetExhausted => {
                        state.budget_hit = true;
                        break;
                    }
                }
            }
        }
        walk_one_root(
            &dir,
            initial_hidden,
            config,
            None,
            &remote,
            &scanned,
            cancel,
            &mut state,
            &mut on_hidden,
        );
        scanned.push(dir);
    }

    finish_count(cancel, &state)
}

struct WalkState {
    started: Instant,
    budget: CountBudget,
    matches: usize,
    visited: usize,
    budget_hit: bool,
}

fn finish_count(cancel: &AtomicBool, state: &WalkState) -> HiddenCount {
    if cancel.load(Ordering::Relaxed) || state.matches == 0 {
        return HiddenCount::default();
    }
    HiddenCount {
        count: state.matches,
        capped: state.budget_hit,
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_one_root(
    root: &Path,
    initial_hidden: bool,
    config: &FilterConfig,
    user_exclude: Option<&Override>,
    remote: &HashSet<PathBuf>,
    skip_dirs: &[PathBuf],
    cancel: &AtomicBool,
    state: &mut WalkState,
    on_hidden: &mut impl FnMut(&Path, &str, bool) -> ProbeMatch,
) {
    let matcher = FilterMatcher::new(root, config.clone());

    // Tier 1: visible directories (to discover hidden files and hidden subdirs) and
    // directories hidden by .gallignore / .ignore / .gitignore.
    let mut tier1: VecDeque<(PathBuf, Arc<DirRules>, bool)> = VecDeque::new();
    // Tier 2: directories hidden by global exclude globs (node_modules, target, ...)
    // or CACHEDIR.TAG — searched only with remaining budget after Tier 1.
    let mut tier2: VecDeque<PathBuf> = VecDeque::new();

    tier1.push_back((
        root.to_path_buf(),
        matcher.root_rules.clone(),
        initial_hidden,
    ));

    'walk: loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if state.matches >= state.budget.max_matches {
            state.budget_hit = true;
            break;
        }
        if state.visited >= state.budget.max_entries
            || state.started.elapsed() >= state.budget.max_duration
        {
            state.budget_hit = true;
            break;
        }

        let (dir, rules, ancestor_hidden, in_tier2) = if let Some((d, r, h)) = tier1.pop_front() {
            (d, Some(r), h, false)
        } else if let Some(d) = tier2.pop_front() {
            (d, None, true, true)
        } else {
            break;
        };

        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };

        for item in rd {
            if state.visited & 63 == 0 {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                if state.started.elapsed() >= state.budget.max_duration {
                    state.budget_hit = true;
                    break 'walk;
                }
            }
            if state.visited >= state.budget.max_entries {
                state.budget_hit = true;
                break 'walk;
            }
            state.visited += 1;

            let Ok(entry) = item else { continue };
            let path = entry.path();
            if skip_dirs.contains(&path) {
                continue;
            }
            if super::index::is_system_skip(&path) || remote.contains(&path) {
                continue;
            }
            let name_os = entry.file_name();
            let name = name_os.to_string_lossy();
            if !config.include_hidden && is_os_hidden(&entry, &name) {
                continue;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            // Never follow symlinks (matches WalkBuilder::follow_links(false)).
            let is_dir = ft.is_dir();

            if let Some(uex) = user_exclude {
                if uex.matched(&path, is_dir).is_ignore() {
                    continue;
                }
            }

            if ancestor_hidden {
                match on_hidden(&path, &name, is_dir) {
                    ProbeMatch::Matched => {
                        state.matches += 1;
                        if state.matches >= state.budget.max_matches {
                            state.budget_hit = true;
                            break 'walk;
                        }
                    }
                    ProbeMatch::NoMatch => {}
                    ProbeMatch::BudgetExhausted => {
                        state.budget_hit = true;
                        break 'walk;
                    }
                }
                if is_dir {
                    if in_tier2 {
                        tier2.push_back(path);
                    } else if let Some(r) = &rules {
                        tier1.push_back((path, r.clone(), true));
                    }
                }
                continue;
            }

            let parent_rules = rules.as_ref().expect("tier1 always has rules");
            match matcher.check_entry(parent_rules, &path, is_dir) {
                None => {
                    // Visible entry: never count it, only descend into visible folders.
                    if is_dir {
                        let child_rules = load_child_rules(&path, parent_rules, config);
                        tier1.push_back((path, child_rules, false));
                    }
                }
                Some(FilterReason::IgnoreFile) => {
                    match on_hidden(&path, &name, is_dir) {
                        ProbeMatch::Matched => {
                            state.matches += 1;
                            if state.matches >= state.budget.max_matches {
                                state.budget_hit = true;
                                break 'walk;
                            }
                        }
                        ProbeMatch::NoMatch => {}
                        ProbeMatch::BudgetExhausted => {
                            state.budget_hit = true;
                            break 'walk;
                        }
                    }
                    if is_dir {
                        tier1.push_back((path, parent_rules.clone(), true));
                    }
                }
                Some(FilterReason::Exclude | FilterReason::Cache) => {
                    match on_hidden(&path, &name, is_dir) {
                        ProbeMatch::Matched => {
                            state.matches += 1;
                            if state.matches >= state.budget.max_matches {
                                state.budget_hit = true;
                                break 'walk;
                            }
                        }
                        ProbeMatch::NoMatch => {}
                        ProbeMatch::BudgetExhausted => {
                            state.budget_hit = true;
                            break 'walk;
                        }
                    }
                    if is_dir {
                        tier2.push_back(path);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Verifies `FilterMatcher` agrees with `ignore::WalkBuilder` on every entry
    /// across `.gallignore`, `.gitignore`, `!` negation, exclude globs, and `CACHEDIR.TAG`.
    #[test]
    fn filter_matcher_matches_walk_builder_parity() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        for dir in [
            ".git",
            "src",
            "gen",
            "dist",
            "node_modules/pkg",
            "cache",
            "sub",
        ] {
            fs::create_dir_all(p.join(dir)).unwrap();
        }
        fs::write(
            p.join("cache/CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .unwrap();
        fs::write(p.join(".gitignore"), "dist/\n*.log\n!keep.log\n").unwrap();
        fs::write(p.join(".gallignore"), "/gen/\n*.tmp\n!important.tmp\n").unwrap();
        fs::write(p.join("sub/.gallignore"), "!local.log\n").unwrap();

        let files = [
            "src/main.rs",
            "gen/api.ts",
            "dist/bundle.js",
            "node_modules/pkg/index.js",
            "cache/item.rs",
            "debug.log",
            "keep.log",
            "scratch.tmp",
            "important.tmp",
            "sub/a.log",
            "sub/local.log",
        ];
        for f in files {
            fs::write(p.join(f), "hello").unwrap();
        }

        let exclude = vec!["node_modules".to_string()];
        // Reference set from WalkBuilder (exact same setup as search.rs / grep.rs).
        let mut wb = ignore::WalkBuilder::new(p);
        wb.standard_filters(false)
            .hidden(true)
            .git_ignore(true)
            .git_global(false)
            .git_exclude(true)
            .ignore(true)
            .require_git(true)
            .follow_links(false)
            .filter_entry(|e| e.depth() == 0 || !super::super::ignorefile::skip_cache(e));
        super::super::ignorefile::configure(&mut wb, true);
        wb.overrides(build_exclude_override(p, &exclude).unwrap());
        let visible_walk: HashSet<PathBuf> = wb
            .build()
            .filter_map(|e| e.ok())
            .filter(|e| e.depth() > 0 && e.file_type().is_some_and(|t| t.is_file()))
            .map(|e| e.into_path())
            .collect();

        let cfg = FilterConfig {
            include_hidden: false,
            respect_gitignore: true,
            exclude,
            use_gallignore: true,
        };
        let matcher = FilterMatcher::new(p, cfg);
        for f in files {
            let full = p.join(f);
            let expected_hidden = !visible_walk.contains(&full);
            assert_eq!(
                matcher.is_path_hidden(&full, false),
                expected_hidden,
                "mismatch for {f}"
            );
        }
    }

    #[test]
    fn gallignore_takes_precedence_over_gitignore_and_works_outside_git() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        fs::create_dir_all(p.join(".git")).unwrap();
        fs::write(p.join(".gitignore"), "*.log\n").unwrap();
        // .gallignore un-ignores special.log even though .gitignore ignores *.log.
        fs::write(p.join(".gallignore"), "!special.log\nsecret.txt\n").unwrap();
        fs::write(p.join("a.log"), "").unwrap();
        fs::write(p.join("special.log"), "").unwrap();
        fs::write(p.join("secret.txt"), "").unwrap();
        fs::write(p.join("normal.txt"), "").unwrap();

        let cfg = FilterConfig {
            include_hidden: false,
            respect_gitignore: true,
            exclude: vec![],
            use_gallignore: true,
        };
        let m = FilterMatcher::new(p, cfg);
        assert!(m.is_path_hidden(&p.join("a.log"), false));
        assert!(!m.is_path_hidden(&p.join("special.log"), false));
        assert!(m.is_path_hidden(&p.join("secret.txt"), false));
        assert!(!m.is_path_hidden(&p.join("normal.txt"), false));

        // Outside a git repo: .gitignore does not apply, .gallignore still applies.
        fs::remove_dir_all(p.join(".git")).unwrap();
        let m2 = FilterMatcher::new(
            p,
            FilterConfig {
                include_hidden: false,
                respect_gitignore: true,
                exclude: vec![],
                use_gallignore: true,
            },
        );
        assert!(!m2.is_path_hidden(&p.join("a.log"), false));
        assert!(m2.is_path_hidden(&p.join("secret.txt"), false));
    }

    #[test]
    fn starting_inside_ignored_folder_keeps_direct_children_visible() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        fs::create_dir_all(p.join("gen")).unwrap();
        fs::write(p.join(".gallignore"), "/gen/\n").unwrap();
        fs::write(p.join("gen/api.ts"), "").unwrap();

        let cfg = FilterConfig {
            include_hidden: false,
            respect_gitignore: false,
            exclude: vec![],
            use_gallignore: true,
        };
        // Searching from parent: gen/api.ts IS hidden.
        let from_parent = FilterMatcher::new(p, cfg.clone());
        assert!(from_parent.is_path_hidden(&p.join("gen/api.ts"), false));

        // Searching directly inside gen/: api.ts is visible (matches WalkBuilder).
        let from_gen = FilterMatcher::new(&p.join("gen"), cfg);
        assert!(!from_gen.is_path_hidden(&p.join("gen/api.ts"), false));
    }

    #[test]
    fn tier1_finds_gallignore_match_before_huge_node_modules_exhausts_budget() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        fs::create_dir_all(p.join("node_modules/big")).unwrap();
        for i in 0..300 {
            fs::write(p.join(format!("node_modules/big/other_{i:03}.js")), "").unwrap();
        }
        fs::create_dir_all(p.join("zz_hidden")).unwrap();
        fs::write(p.join("zz_hidden/needle.txt"), "").unwrap();
        fs::write(p.join(".gallignore"), "/zz_hidden/\n").unwrap();

        let cfg = FilterConfig {
            include_hidden: false,
            respect_gitignore: false,
            exclude: vec!["node_modules".into()],
            use_gallignore: true,
        };
        // Budget too small to scan all 300 node_modules files, yet Tier 1 visits
        // zz_hidden first and finds needle.txt before node_modules is entered.
        let budget = CountBudget {
            max_matches: 100,
            max_entries: 50,
            max_duration: Duration::from_secs(2),
        };
        let cancel = AtomicBool::new(false);
        let res = walk_hidden_matches(p, &cfg, None, budget, &cancel, |_path, name, _dir| {
            if name == "needle.txt" {
                ProbeMatch::Matched
            } else {
                ProbeMatch::NoMatch
            }
        });
        assert_eq!(res.count, 1);
    }

    #[test]
    fn budget_caps_large_hidden_match_count() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        fs::create_dir_all(p.join("node_modules/pkg")).unwrap();
        for i in 0..50 {
            fs::write(p.join(format!("node_modules/pkg/hit_{i}.js")), "").unwrap();
        }
        let cfg = FilterConfig {
            include_hidden: false,
            respect_gitignore: false,
            exclude: vec!["node_modules".into()],
            use_gallignore: true,
        };
        let budget = CountBudget {
            max_matches: 10,
            max_entries: 10_000,
            max_duration: Duration::from_secs(2),
        };
        let cancel = AtomicBool::new(false);
        let res = walk_hidden_matches(p, &cfg, None, budget, &cancel, |_path, name, _dir| {
            if name.starts_with("hit_") {
                ProbeMatch::Matched
            } else {
                ProbeMatch::NoMatch
            }
        });
        assert_eq!(
            res,
            HiddenCount {
                count: 10,
                capped: true
            }
        );
    }
}
