//! What is known about a repository without starting Git.
//!
//! A Git process costs a few milliseconds on Linux and 40–80 ms on Windows, and the views
//! used to ask the same cheap questions again on every call: where the work tree starts,
//! where its Git directory is, whether an operation is in progress, whether the
//! configuration defines content filters. Here the first three are read from the files
//! Git keeps, and answers Git gave are kept for as long as the files they came from are
//! unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Entries kept per cache; when one is full it starts again.
const CAPACITY: usize = 512;

type Roots = HashMap<PathBuf, (PathBuf, PathBuf)>;
static ROOTS: Mutex<Option<Roots>> = Mutex::new(None);

/// The work tree root Git reports for `dir` (`ask` runs `rev-parse --show-toplevel`),
/// asked once per folder. The answer is reused while the nearest folder with a `.git`
/// stays the same one, which is checked without a process; Git's own spelling of the path
/// (resolved symlinks) is kept that way.
pub(super) fn root(dir: &Path, ask: impl FnOnce() -> Option<PathBuf>) -> Option<PathBuf> {
    let tree = super::gitstatus::work_tree(dir)?;
    if let Ok(cache) = ROOTS.lock() {
        if let Some((known, root)) = cache.as_ref().and_then(|c| c.get(dir)) {
            if *known == tree && same_folder(&tree, root) {
                super::gitstatus::accept(root);
                return Some(root.clone());
            }
        }
    }
    let root = ask()?;
    // Git answered: it does not refuse this repository.
    super::gitstatus::accept(&root);
    if let Ok(mut cache) = ROOTS.lock() {
        let cache = cache.get_or_insert_with(HashMap::new);
        if cache.len() >= CAPACITY {
            cache.clear();
        }
        cache.insert(dir.to_path_buf(), (tree, root.clone()));
    }
    Some(root)
}

/// Whether the folder with the `.git` (`tree`) is still the root Git named. They differ in
/// spelling when a link leads to the repository, and in substance when the `.git` found
/// is not a repository (Git then answers with an outer one) or the link was pointed
/// elsewhere; a root that is not the same folder is asked for again.
fn same_folder(tree: &Path, root: &Path) -> bool {
    tree == root
        || matches!(
            (tree.canonicalize(), root.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}

/// The per-worktree Git directory read from `.git` (a folder, or a `gitdir:` file for
/// linked worktrees and submodules). None when `root` is not a work tree root.
pub(super) fn git_dir(root: &Path) -> Option<PathBuf> {
    let dot = root.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    let text = std::fs::read_to_string(&dot).ok()?;
    let target = Path::new(text.lines().next()?.strip_prefix("gitdir:")?.trim());
    Some(root.join(target))
}

/// Files and folders Git keeps in the per-worktree Git directory while an operation runs.
const PER_WORKTREE: [&str; 9] = [
    "MERGE_HEAD",
    "MERGE_MSG",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "REBASE_HEAD",
    "BISECT_START",
    "rebase-merge",
    "rebase-apply",
    "sequencer",
];

/// Where `name` (what `rev-parse --git-path` takes) is for the work tree `root`, when it
/// is one of the per-worktree operation markers and the Git directory could be read.
pub(super) fn marker(root: &Path, name: &str) -> Option<PathBuf> {
    PER_WORKTREE
        .contains(&name.split('/').next().unwrap_or(name))
        .then(|| git_dir(root))?
        .map(|dir| dir.join(name))
}

/// The operation markers of a work tree, from one listing of its Git directory. A view
/// looks for a dozen markers that normally do not exist, and through WSL's file server
/// (`\\wsl.localhost\…`) looking up a missing file takes about 25 ms while listing the
/// folder takes about 3 ms. Only a marker whose first part is listed is looked up, so
/// [`Markers::exists`] and [`Markers::read`] answer as `Path::exists` and `fs::read` do.
pub(super) struct Markers {
    dir: PathBuf,
    /// Names in the folder; lower case where file names ignore case.
    names: std::collections::HashSet<String>,
}

const FOLDED_NAMES: bool = cfg!(any(windows, target_os = "macos"));

impl Markers {
    fn listed(&self, name: &str) -> bool {
        let first = name.split('/').next().unwrap_or(name);
        if FOLDED_NAMES {
            self.names.contains(&first.to_lowercase())
        } else {
            self.names.contains(first)
        }
    }

    pub(super) fn exists(&self, name: &str) -> bool {
        self.listed(name) && self.dir.join(name).exists()
    }

    pub(super) fn read(&self, name: &str) -> Option<Vec<u8>> {
        self.listed(name)
            .then(|| std::fs::read(self.dir.join(name)).ok())
            .flatten()
    }
}

/// [`Markers`] of the work tree `root`; None when its Git directory cannot be listed.
pub(super) fn markers(root: &Path) -> Option<Markers> {
    let dir = git_dir(root)?;
    let names = std::fs::read_dir(&dir)
        .ok()?
        .map(|entry| {
            entry.map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if FOLDED_NAMES {
                    name.to_lowercase()
                } else {
                    name
                }
            })
        })
        .collect::<std::io::Result<_>>()
        .ok()?;
    Some(Markers { dir, names })
}

/// The Git directory shared by all work trees of the repository (objects, references).
fn common_dir(root: &Path) -> Option<PathBuf> {
    let dir = git_dir(root)?;
    Some(match std::fs::read_to_string(dir.join("commondir")) {
        Ok(common) => dir.join(common.trim()),
        Err(_) => dir,
    })
}

/// The per-worktree and the common Git directory of `root` when its references can be read
/// from files instead of asking Git: a repository on this machine (WSL has its own Git and a
/// slow way to its files), with references stored as files (not reftable), and no variable
/// in the environment that sends Git to another repository.
fn ref_files(root: &Path) -> Option<(PathBuf, PathBuf)> {
    if super::gitexec::place(root).is_some()
        || [
            "GIT_DIR",
            "GIT_COMMON_DIR",
            "GIT_WORK_TREE",
            "GIT_NAMESPACE",
        ]
        .iter()
        .any(|key| std::env::var_os(key).is_some())
    {
        return None;
    }
    let dir = git_dir(root)?;
    let common = common_dir(root)?;
    (dir.join("HEAD").is_file() && !common.join("reftable").exists()).then_some((dir, common))
}

/// References that belong to one work tree and live in its own Git directory.
const WORKTREE_REFS: [&str; 3] = ["refs/bisect/", "refs/worktree/", "refs/rewritten/"];

fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The value stored for reference `name`: an object id or `ref: <target>`. A loose file comes
/// before `packed` (the content of `packed-refs`). None when there is no such reference.
fn stored(name: &str, dir: &Path, common: &Path, packed: &str) -> Option<String> {
    let base = if WORKTREE_REFS.iter().any(|p| name.starts_with(p)) {
        dir
    } else {
        common
    };
    let file = base.join(name);
    if file.is_file() {
        return std::fs::read_to_string(file)
            .ok()
            .map(|text| text.trim_end().to_owned());
    }
    packed
        .lines()
        .filter(|line| !line.starts_with(['#', '^']))
        .find_map(|line| {
            let (id, entry) = line.split_once(' ')?;
            (entry == name).then(|| id.to_owned())
        })
}

/// The object id `name` leads to, following symbolic references.
fn resolve(name: &str, dir: &Path, common: &Path, packed: &str) -> Option<String> {
    let mut name = name.to_owned();
    for _ in 0..5 {
        let value = stored(&name, dir, common, packed)?;
        match value.strip_prefix("ref:") {
            Some(target) => name = target.trim().to_owned(),
            None => return is_object_id(&value).then_some(value),
        }
    }
    None
}

/// The commit HEAD points to, as `git rev-parse --verify HEAD` prints it, read from files.
/// None where Git must be asked (see [`ref_files`]), and before the first commit.
pub(super) fn head_id(root: &Path) -> Option<String> {
    let (dir, common) = ref_files(root)?;
    let text = std::fs::read_to_string(dir.join("HEAD")).ok()?;
    let text = text.trim_end();
    match text.strip_prefix("ref:") {
        Some(target) => {
            let packed = std::fs::read_to_string(common.join("packed-refs")).unwrap_or_default();
            resolve(target.trim(), &dir, &common, &packed)
        }
        None => is_object_id(text).then(|| text.to_owned()),
    }
}

/// HEAD as `git rev-parse HEAD --symbolic-full-name HEAD --abbrev-ref HEAD` reports it: the
/// commit, the reference (`refs/heads/<branch>`, or `HEAD` when detached) and the branch.
/// Read from files; None where Git must be asked: before the first commit, when HEAD points
/// outside `refs/heads/`, and when another reference has the branch's short name (Git then
/// prints a longer one).
pub(super) fn head(root: &Path) -> Option<(String, String, Option<String>)> {
    let (dir, common) = ref_files(root)?;
    let text = std::fs::read_to_string(dir.join("HEAD")).ok()?;
    let text = text.trim_end();
    let Some(target) = text.strip_prefix("ref:") else {
        return is_object_id(text).then(|| (text.to_owned(), "HEAD".to_owned(), None));
    };
    let target = target.trim();
    let branch = target.strip_prefix("refs/heads/")?;
    let packed = std::fs::read_to_string(common.join("packed-refs")).unwrap_or_default();
    // A branch that is itself symbolic: Git names the branch at the end of the chain.
    let id = stored(target, &dir, &common, &packed).filter(|id| is_object_id(id))?;
    let taken = short_name_taken(branch, &dir, |name| {
        stored(name, &dir, &common, &packed).is_some()
    });
    (!taken).then(|| (id, target.to_owned(), Some(branch.to_owned())))
}

/// Whether HEAD is detached, read from its file: `Some(None)` when it names a reference
/// below `refs/` (what `git symbolic-ref -q HEAD` succeeds for, also before the first
/// commit), `Some(Some(id))` when it holds a commit id. None where Git must be asked (see
/// [`ref_files`]) or the file says something else.
pub(super) fn detached_head(root: &Path) -> Option<Option<String>> {
    let (dir, _) = ref_files(root)?;
    let text = std::fs::read_to_string(dir.join("HEAD")).ok()?;
    let text = text.trim_end();
    match text.strip_prefix("ref:") {
        Some(target) => target.trim().starts_with("refs/").then_some(None),
        None => is_object_id(text).then(|| Some(text.to_owned())),
    }
}

/// Whether another reference could answer to the short name of branch `branch` (`exists`
/// says whether a full reference name exists). Git then shortens the branch to `heads/<b>`.
fn short_name_taken(branch: &str, dir: &Path, exists: impl Fn(&str) -> bool) -> bool {
    // The places gitrevisions(7) tries for a short name, other than the branch itself.
    (!branch.contains('/') && dir.join(branch).exists())
        || [
            format!("refs/{branch}"),
            format!("refs/tags/{branch}"),
            format!("refs/remotes/{branch}"),
            format!("refs/remotes/{branch}/HEAD"),
        ]
        .iter()
        .any(|name| exists(name))
}

/// Whether `name` (below `refs/`) is a reference name Git lists, as git-check-ref-format(1)
/// describes it. Git skips other names; the callers leave such repositories to Git. A full
/// name (`refs/heads/<b>`) passes exactly when `git check-ref-format <name>` accepts it.
pub(super) fn plain_ref_name(name: &str) -> bool {
    !name.is_empty()
        && name != "@"
        && !name.contains("..")
        && !name.contains("@{")
        && !name.ends_with(['/', '.'])
        && !name.bytes().any(|b| {
            b < 0x20
                || matches!(
                    b,
                    0x7f | b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\'
                )
        })
        && name
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

/// The local branches as `git for-each-ref --format=%(refname:short) refs/heads/` lists
/// them, read from files. None where Git must be asked (see [`ref_files`]) or where its
/// answer could differ: a branch that is symbolic or unreadable, one with a name Git would
/// not list or shorten to `heads/<b>`, and, on macOS, names that are not ASCII (the folder
/// listing may give them in another Unicode form than Git stores).
pub(super) fn branches(root: &Path) -> Option<Vec<String>> {
    let (dir, common) = ref_files(root)?;
    let refs = ref_map(&dir, &common);
    // Git finds a loose reference by opening its file, which ignores letter case on Windows
    // and (by default) on macOS: `refs/tags/Main` hides the short name of branch `main`.
    let folded: Option<std::collections::HashSet<String>> = cfg!(any(windows, target_os = "macos"))
        .then(|| refs.keys().map(|name| name.to_lowercase()).collect());
    let exists = |name: &str| {
        refs.contains_key(name)
            || folded
                .as_ref()
                .is_some_and(|folded| folded.contains(&name.to_lowercase()))
    };
    let mut names = Vec::new();
    for (name, value) in refs.range::<str, _>((
        std::ops::Bound::Included("refs/heads/"),
        std::ops::Bound::Unbounded,
    )) {
        let Some(branch) = name.strip_prefix("refs/heads/") else {
            break;
        };
        if !is_object_id(value)
            || !plain_ref_name(branch)
            || (cfg!(target_os = "macos") && !branch.is_ascii())
            || short_name_taken(branch, &dir, exists)
        {
            return None;
        }
        names.push(branch.to_owned());
    }
    Some(names)
}

/// The stash list as `git stash list --format=%H%x00%gd%x00%gs` prints it, newest first:
/// commit, `stash@{<n>}` and message. Read from the reflog of `refs/stash`; None where Git
/// must be asked (see [`ref_files`]) or the files do not agree with each other.
pub(super) fn stashes(root: &Path) -> Option<Vec<(String, String, String)>> {
    let (dir, common) = ref_files(root)?;
    let packed = std::fs::read_to_string(common.join("packed-refs")).unwrap_or_default();
    let current = stored("refs/stash", &dir, &common, &packed);
    let log = match std::fs::read(common.join("logs").join("refs").join("stash")) {
        Ok(log) => String::from_utf8(log).ok()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && current.is_none() => {
            return Some(Vec::new());
        }
        Err(_) => return None,
    };
    let mut entries = Vec::new();
    for line in log.lines() {
        // `<old> <new> <who> <time> <zone>\t<message>`
        let (fields, message) = line.split_once('\t')?;
        let new = fields.split(' ').nth(1)?;
        if !is_object_id(new) || new.bytes().all(|b| b == b'0') {
            return None;
        }
        entries.push((new.to_owned(), message.to_owned()));
    }
    // The reference names the newest entry; anything else is a state Git should explain.
    if entries.last().map(|(id, _)| id) != current.as_ref() {
        return None;
    }
    Some(
        entries
            .into_iter()
            .rev()
            .enumerate()
            .map(|(n, (id, message))| (id, format!("stash@{{{n}}}"), message))
            .collect(),
    )
}

/// What the files of a repository say about its references, local configuration and stash
/// list. Equal values mean nothing of that changed; it is for noticing changes, not for
/// reading values.
#[derive(Hash)]
pub(super) struct RepositoryFiles {
    /// Every reference by full name with its stored value; loose files over `packed-refs`.
    refs: std::collections::BTreeMap<String, String>,
    config: Option<Vec<u8>>,
    worktree_config: Option<Vec<u8>>,
    stash_log: Option<Vec<u8>>,
}

/// Reads [`RepositoryFiles`] for the work tree `root`; None where Git must be asked (see
/// [`ref_files`]).
pub(super) fn repository_files(root: &Path) -> Option<RepositoryFiles> {
    let (dir, common) = ref_files(root)?;
    let refs = ref_map(&dir, &common);
    let read = |file: PathBuf| std::fs::read(file).ok();
    Some(RepositoryFiles {
        refs,
        config: read(common.join("config")),
        worktree_config: read(dir.join("config.worktree")),
        stash_log: read(common.join("logs").join("refs").join("stash")),
    })
}

/// Every reference by full name with its stored value; loose files over `packed-refs`.
fn ref_map(dir: &Path, common: &Path) -> std::collections::BTreeMap<String, String> {
    let mut refs = std::collections::BTreeMap::new();
    let packed = std::fs::read_to_string(common.join("packed-refs")).unwrap_or_default();
    for line in packed.lines().filter(|l| !l.starts_with(['#', '^'])) {
        if let Some((id, name)) = line.split_once(' ') {
            refs.insert(name.to_owned(), id.to_owned());
        }
    }
    loose_refs(&common.join("refs"), "refs", &mut refs);
    if dir != common {
        loose_refs(&dir.join("refs"), "refs", &mut refs);
    }
    refs
}

fn loose_refs(folder: &Path, prefix: &str, refs: &mut std::collections::BTreeMap<String, String>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        // A reference being written (`<name>.lock`) is not one yet.
        if file_name.ends_with(".lock") {
            continue;
        }
        let name = format!("{prefix}/{file_name}");
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => loose_refs(&entry.path(), &name, refs),
            Ok(_) => {
                if let Some(text) = loose_ref(&entry) {
                    refs.insert(name, text);
                }
            }
            Err(_) => {}
        }
    }
}

type LooseRefs = HashMap<PathBuf, (u64, SystemTime, String)>;
static LOOSE_REFS: Mutex<Option<LooseRefs>> = Mutex::new(None);

/// The content of a loose reference file. Git replaces such a file (it writes a new one and
/// renames it over the old), which gives it a new modification time, so the content read
/// before is reused while size and time are the same. A repository can have thousands of
/// these files and a view reads them twice; the size and time come with the folder listing.
///
/// File times have a coarse tick (about 15 ms on Windows, a few on Linux), so two writes in
/// one tick could share a time. A file written in the last [`REF_SETTLE`] is therefore always
/// read: once the cached content was read after that, a later write has a later time.
fn loose_ref(entry: &std::fs::DirEntry) -> Option<String> {
    let path = entry.path();
    let meta = entry.metadata().ok()?;
    let key = (meta.len(), meta.modified().ok()?);
    let settled = key.1.elapsed().is_ok_and(|age| age >= REF_SETTLE);
    if settled {
        if let Ok(cache) = LOOSE_REFS.lock() {
            if let Some((len, time, text)) = cache.as_ref().and_then(|c| c.get(&path)) {
                if (*len, *time) == key {
                    return Some(text.clone());
                }
            }
        }
    }
    let text = std::fs::read_to_string(&path).ok()?.trim_end().to_owned();
    if settled {
        if let Ok(mut cache) = LOOSE_REFS.lock() {
            let cache = cache.get_or_insert_with(HashMap::new);
            if cache.len() >= 64 * CAPACITY {
                cache.clear();
            }
            cache.insert(path, (key.0, key.1, text.clone()));
        }
    }
    Some(text)
}

/// How old a reference file must be before its content is reused (see [`loose_ref`]).
const REF_SETTLE: Duration = Duration::from_secs(2);

/// How long a commit-graph is left alone, however many commits were made since.
const GRAPH_FRESH_FOR: Duration = Duration::from_secs(3600);
/// How often a repository is looked at again while the app runs.
const GRAPH_LOOK_EVERY: Duration = Duration::from_secs(600);

/// Whether the repository has no commit-graph with changed-path filters, or one that is
/// both older than [`GRAPH_FRESH_FOR`] and older than the last movement of HEAD.
fn commit_graph_stale(root: &Path) -> bool {
    let Some(common) = common_dir(root) else {
        return false;
    };
    let info = common.join("objects").join("info");
    let modified = |p: &Path| p.metadata().ok().and_then(|m| m.modified().ok());
    // A chain of graph files (`fetch.writeCommitGraph`, `git maintenance`) is kept up by
    // Git itself.
    if info
        .join("commit-graphs")
        .join("commit-graph-chain")
        .exists()
    {
        return false;
    }
    let file = info.join("commit-graph");
    let Some(written) = modified(&file) else {
        return true;
    };
    // The chunk table at the start names a changed-path index (`BIDX`) when there is one.
    let mut head = [0u8; 256];
    let filters = std::fs::File::open(&file)
        .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
        .is_ok_and(|n| head[..n].windows(4).any(|w| w == b"BIDX"));
    if !filters {
        return true;
    }
    let old = written.elapsed().is_ok_and(|age| age > GRAPH_FRESH_FOR);
    let moved = [common.join("logs").join("HEAD"), common.join("packed-refs")]
        .iter()
        .filter_map(|p| modified(p))
        .any(|at| at > written);
    old && moved
}

/// Writes the commit-graph of the repository at `root` with changed-path filters when it
/// is missing or stale; waits for Git. The file is an index Git itself maintains
/// (`git gc`, `git maintenance`): it changes no commit, reference or file of the work
/// tree, and Git ignores it when it does not fit.
///
/// With it Git orders the history without reading every commit and finds the commits of
/// one file without comparing every tree: in a repository of 20 000 commits the history
/// graph takes 40 ms instead of 230 ms, the history of a file 25 ms instead of 200 ms and
/// blame 55 ms instead of 170 ms. Writing it takes a few seconds there, once.
pub(super) fn write_commit_graph(root: &Path) {
    mark_graph_looked_at(root);
    refresh_index(root);
    if commit_graph_stale(root) {
        // New filters are limited per run, so a huge history is indexed over several runs
        // instead of in one that takes minutes.
        let _ = super::gitgraph::git(
            root,
            &[
                "commit-graph",
                "write",
                "--reachable",
                "--changed-paths",
                "--max-new-filters=100000",
                "--no-progress",
            ],
        );
    }
}

/// Lets Git bring the file times in the index of the work tree `root` up to date, as an
/// ordinary `git status` in a terminal does.
///
/// Gall's own queries never write the index (they must not get in the way of a Git
/// running at the same time). But after a clone, a checkout or `git worktree add` the
/// index cannot tell whether a file written in the same moment as itself changed, and
/// until it is written again Git compares the content of every such file on every
/// status: 250 ms instead of 100 ms in a work tree of 50 000 files. So the index is
/// refreshed once in a while, in the background, with hooks switched off. Not in a
/// repository with content filters, where comparing content would run them.
fn refresh_index(root: &Path) {
    if super::gitgraph::has_content_filters(root) {
        return;
    }
    // With a file system monitor configured the index holds its state, which a refresh
    // run without the monitor (as all of Gall's Git runs are) would drop.
    let monitor = super::gitgraph::git(root, &["config", "--get", "core.fsmonitor"]);
    if monitor.is_ok_and(|value| !matches!(value.trim(), "" | "false" | "0" | "no" | "off")) {
        return;
    }
    // Not while one of Gall's own operations runs: both would want the index lock, and
    // an operation that fails on it half way (a rebase) is worse than a slow status.
    let Ok(_alone) = BUSY.try_write() else {
        return;
    };
    // Exits with 1 when files are modified; that is no failure.
    let _ = super::gitgraph::git(root, &["update-index", "-q", "--refresh"]);
}

/// Held shared by every operation of Gall that changes a repository, and alone by the
/// background index refresh, so the two never run at the same time.
static BUSY: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// Marks an operation that changes a repository as running until the result is dropped.
/// Waits for a background index refresh that is under way (a fraction of a second).
pub(super) fn mutating() -> std::sync::RwLockReadGuard<'static, ()> {
    BUSY.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

static GRAPHS: Mutex<Option<HashMap<PathBuf, Instant>>> = Mutex::new(None);

/// Notes that the commit-graph of `root` is being looked after now; false when it was
/// looked at less than [`GRAPH_LOOK_EVERY`] ago.
fn mark_graph_looked_at(root: &Path) -> bool {
    let Ok(mut looked) = GRAPHS.lock() else {
        return false;
    };
    let looked = looked.get_or_insert_with(HashMap::new);
    if looked
        .get(root)
        .is_some_and(|at| at.elapsed() < GRAPH_LOOK_EVERY)
    {
        return false;
    }
    if looked.len() >= CAPACITY {
        looked.clear();
    }
    looked.insert(root.to_path_buf(), Instant::now());
    true
}

/// Keeps the commit-graph and the index of the work tree at `root` fresh (see
/// [`write_commit_graph`] and [`refresh_index`]) without making the caller wait: called
/// when a Git view opens, it starts the work in the background, at most once per
/// [`GRAPH_LOOK_EVERY`] for a work tree.
pub(super) fn keep_commit_graph(root: &Path) {
    // The unit tests build and delete repositories faster than a background Git would end.
    if cfg!(test) || !mark_graph_looked_at(root) {
        return;
    }
    let root = root.to_path_buf();
    std::thread::spawn(move || write_commit_graph(&root));
}

/// Size and modification time of the configuration files a repository reads.
type Fingerprint = Vec<Option<(u64, SystemTime)>>;

fn config_fingerprint(root: &Path) -> Fingerprint {
    let print = |meta: std::fs::Metadata| Some((meta.len(), meta.modified().ok()?));
    let mut found = Vec::new();
    let mut files = Vec::new();
    if let Some(dir) = git_dir(root) {
        // The files of the Git directory come from one listing of it: two of them usually do
        // not exist, and looking up a missing file is slow through WSL's file server.
        let mut listed: HashMap<String, std::fs::DirEntry> = HashMap::new();
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let mut name = entry.file_name().to_string_lossy().into_owned();
            if FOLDED_NAMES {
                name = name.to_lowercase();
            }
            if matches!(name.as_str(), "config" | "config.worktree" | "commondir") {
                listed.insert(name, entry);
            }
        }
        let meta = |name: &str| {
            let entry = listed.get(name)?;
            // A link is followed, as `Path::metadata` does.
            match entry.file_type().ok()?.is_symlink() {
                true => entry.path().metadata().ok(),
                false => entry.metadata().ok(),
            }
        };
        found.push(meta("config").and_then(print));
        found.push(meta("config.worktree").and_then(print));
        // A linked worktree: the shared configuration is in the common directory.
        if listed.contains_key("commondir") {
            if let Ok(common) = std::fs::read_to_string(dir.join("commondir")) {
                files.push(dir.join(common.trim()).join("config"));
            }
        }
    }
    if let Some(global) = std::env::var_os("GIT_CONFIG_GLOBAL") {
        files.push(global.into());
    }
    if let Some(home) = dirs::home_dir() {
        files.push(home.join(".gitconfig"));
        let xdg = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        files.push(xdg.join("git").join("config"));
    }
    found.extend(files.iter().map(|f| f.metadata().ok().and_then(print)));
    found
}

/// The content filter settings of a repository: `(key, value)` of every
/// `filter.<name>.(clean|smudge|process)`.
pub(super) type Filters = Arc<Vec<(String, String)>>;

/// How long a filter answer may be reused although the files checked are unchanged:
/// included files and the system configuration are not among them.
const FILTERS_FOR: Duration = Duration::from_secs(10);

type FilterCache = HashMap<PathBuf, (Fingerprint, Instant, Filters)>;
static FILTERS: Mutex<Option<FilterCache>> = Mutex::new(None);

/// The content filters in the effective configuration of the repository at `root`. Git is
/// asked (one `config` process) when the repository's or the user's configuration file
/// changed, and at least every [`FILTERS_FOR`]. A failed query is no filters, as before.
///
/// An answer is kept only for a repository Git has accepted (a status or a root query
/// worked). While Git refuses a repository ("dubious ownership") it ignores that
/// repository's configuration and reports no filters; keeping that answer would let the
/// filters run in the first status after the repository is trusted.
pub(super) fn filters(root: &Path) -> Filters {
    let print = config_fingerprint(root);
    if let Ok(cache) = FILTERS.lock() {
        if let Some((known, at, filters)) = cache.as_ref().and_then(|c| c.get(root)) {
            if *known == print && at.elapsed() < FILTERS_FOR {
                return filters.clone();
            }
        }
    }
    let asked = Instant::now();
    let out = super::gitgraph::git(
        root,
        &[
            "config",
            "-z",
            "--get-regexp",
            r"^filter\..*\.(clean|smudge|process)$",
        ],
    );
    let filters: Filters = Arc::new(parse_filters(&out.unwrap_or_default()));
    if super::gitstatus::accepted(root) {
        if let Ok(mut cache) = FILTERS.lock() {
            let cache = cache.get_or_insert_with(HashMap::new);
            if cache.len() >= CAPACITY {
                cache.clear();
            }
            cache.insert(root.to_path_buf(), (print, asked, filters.clone()));
        }
    }
    filters
}

/// `git config -z --get-regexp` output: `key\nvalue\0` per setting.
fn parse_filters(out: &str) -> Vec<(String, String)> {
    out.split('\0')
        .filter(|record| !record.is_empty())
        .map(|record| {
            let (key, value) = record.split_once('\n').unwrap_or((record, ""));
            (key.to_owned(), value.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_records_are_split_into_key_and_value() {
        let parsed = parse_filters("filter.lfs.clean\ngit-lfs clean -- %f\0filter.a b.process\n\0");
        assert_eq!(
            parsed,
            vec![
                (
                    "filter.lfs.clean".to_owned(),
                    "git-lfs clean -- %f".to_owned()
                ),
                ("filter.a b.process".to_owned(), String::new()),
            ]
        );
        assert!(parse_filters("").is_empty());
    }

    #[test]
    fn markers_and_the_git_directory_are_read_from_dot_git() {
        let t = tempfile::tempdir().unwrap();
        let main = t.path().join("main");
        std::fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
        assert_eq!(git_dir(&main), Some(main.join(".git")));
        assert_eq!(
            marker(&main, "MERGE_HEAD"),
            Some(main.join(".git/MERGE_HEAD"))
        );
        assert_eq!(
            marker(&main, "rebase-merge/done"),
            Some(main.join(".git/rebase-merge/done"))
        );
        assert_eq!(marker(&main, "hooks"), None, "shared, not per worktree");
        // A linked worktree keeps its markers in its own directory.
        let linked = t.path().join("linked");
        std::fs::create_dir(&linked).unwrap();
        let target = main.join(".git/worktrees/wt");
        std::fs::write(
            linked.join(".git"),
            format!("gitdir: {}\n", target.display()),
        )
        .unwrap();
        assert_eq!(git_dir(&linked), Some(target.clone()));
        assert_eq!(
            marker(&linked, "BISECT_START"),
            Some(target.join("BISECT_START"))
        );
        assert_eq!(git_dir(t.path()), None);
    }

    /// A repository with one commit, or None when Git is not installed.
    fn repository(dir: &Path) -> Option<()> {
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .is_ok_and(|out| out.status.success())
        };
        run(&["init", "-q"]).then_some(())?;
        run(&["config", "core.autocrlf", "false"]);
        Some(())
    }

    fn kept(root: &Path) -> bool {
        FILTERS
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|cache| cache.contains_key(root))
    }

    #[test]
    fn filters_are_kept_only_for_an_accepted_repository_and_follow_the_configuration() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        if repository(&root).is_none() {
            eprintln!("git not installed: skipped");
            return;
        }
        // Not accepted yet (Git may be refusing it and hiding its filters): asked, not kept.
        // Git for Windows and developer machines may already configure LFS or other
        // filters globally. Keep that baseline: the cache must preserve it too.
        let inherited = filters(&root);
        assert!(
            !kept(&root),
            "an answer about an unaccepted repository was kept"
        );
        super::super::gitstatus::accept(&root);
        assert_eq!(*filters(&root), *inherited);
        assert!(kept(&root));
        // A filter added to the configuration is seen at once, not after the time limit.
        let set = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["config", "filter.evil.clean", "touch ran"])
            .status()
            .unwrap();
        assert!(set.success());
        let found = filters(&root);
        let mut expected = (*inherited).clone();
        expected.push(("filter.evil.clean".to_owned(), "touch ran".to_owned()));
        assert_eq!(*found, expected);
    }

    /// Runs Git in `dir` with a fixed author; the output, or None when it failed.
    fn run_git(dir: &Path, args: &[&str]) -> Option<String> {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
            .arg(dir)
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// What Git says about HEAD, in the form of [`head`].
    fn git_head(dir: &Path) -> Option<(String, String, Option<String>)> {
        let out = run_git(
            dir,
            &[
                "rev-parse",
                "HEAD",
                "--symbolic-full-name",
                "HEAD",
                "--abbrev-ref",
                "HEAD",
            ],
        )?;
        let [id, reference, branch] = out.lines().collect::<Vec<_>>()[..] else {
            return None;
        };
        Some((
            id.into(),
            reference.into(),
            (reference != "HEAD").then(|| branch.into()),
        ))
    }

    #[test]
    fn head_read_from_files_matches_git_or_leaves_the_question_to_git() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        if run_git(&root, &["init", "-q", "-b", "main"]).is_none() {
            eprintln!("git not installed: skipped");
            return;
        }
        assert_eq!(head(&root), None, "before the first commit Git is asked");
        assert_eq!(head_id(&root), None, "before the first commit Git is asked");
        run_git(&root, &["commit", "-q", "--allow-empty", "-m", "one"]).unwrap();
        let same = |why: &str| {
            let read = head(&root);
            assert!(read.is_some(), "{why}: not read from files");
            assert_eq!(read, git_head(&root), "{why}");
            assert_eq!(head_id(&root), read.map(|(id, _, _)| id), "{why}: id");
        };
        same("loose branch");
        run_git(&root, &["switch", "-q", "-c", "topic/a"]).unwrap();
        run_git(&root, &["pack-refs", "--all"]).unwrap();
        same("packed branch with a slash");
        run_git(&root, &["switch", "-q", "--detach"]).unwrap();
        same("detached");
        run_git(&root, &["switch", "-q", "main"]).unwrap();
        // Another reference with the branch's short name: Git prints a longer name.
        for other in ["refs/tags/main", "refs/remotes/main"] {
            run_git(&root, &["update-ref", other, "HEAD"]).unwrap();
            assert_eq!(head(&root), None, "{other}");
            assert_eq!(
                head_id(&root),
                git_head(&root).map(|(id, _, _)| id),
                "{other}"
            );
            run_git(&root, &["update-ref", "-d", other]).unwrap();
        }
        same("ambiguity removed");
        // A linked worktree keeps its own HEAD.
        let linked = t.path().join("linked");
        let linked_arg = linked.to_string_lossy().into_owned();
        run_git(&root, &["worktree", "add", "-q", "-b", "side", &linked_arg]).unwrap();
        run_git(&linked, &["commit", "-q", "--allow-empty", "-m", "two"]).unwrap();
        let read = head(&linked);
        assert!(read.is_some());
        assert_eq!(read, git_head(&linked));
        assert_ne!(read, head(&root));
    }

    #[test]
    fn repository_files_change_with_references_configuration_and_stashes_only() {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        if run_git(&root, &["init", "-q", "-b", "main"]).is_none() {
            eprintln!("git not installed: skipped");
            return;
        }
        run_git(&root, &["config", "core.autocrlf", "false"]).unwrap();
        std::fs::write(root.join("f"), "1").unwrap();
        run_git(&root, &["add", "f"]).unwrap();
        run_git(&root, &["commit", "-q", "-m", "one"]).unwrap();
        let print = || {
            let mut h = DefaultHasher::new();
            repository_files(&root)
                .expect("read from files")
                .hash(&mut h);
            h.finish()
        };
        let mut last = print();
        let mut changed = |why: &str, step: &dyn Fn()| {
            step();
            let now = print();
            assert_ne!(now, last, "{why}: not noticed");
            last = now;
        };
        changed("new branch", &|| {
            run_git(&root, &["branch", "b"]).unwrap();
        });
        changed("tag", &|| {
            run_git(&root, &["tag", "v1"]).unwrap();
        });
        changed("configuration", &|| {
            run_git(&root, &["config", "branch.b.remote", "origin"]).unwrap();
        });
        changed("stash", &|| {
            std::fs::write(root.join("f"), "2").unwrap();
            run_git(&root, &["stash", "-q"]).unwrap();
        });
        changed("second stash", &|| {
            std::fs::write(root.join("f"), "3").unwrap();
            run_git(&root, &["stash", "-q"]).unwrap();
        });
        changed("stash dropped", &|| {
            run_git(&root, &["stash", "drop", "-q"]).unwrap();
        });
        let before = print();
        run_git(&root, &["pack-refs", "--all"]).unwrap();
        assert_eq!(print(), before, "packing changes no reference");
        std::fs::write(root.join("f"), "4").unwrap();
        assert_eq!(print(), before, "a work tree edit is not part of it");
        changed("packed branch moved", &|| {
            run_git(&root, &["update-ref", "refs/heads/b", "refs/stash"]).unwrap();
        });
    }

    #[test]
    fn branches_and_stashes_read_from_files_match_git_or_are_left_to_git() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        if run_git(&root, &["init", "-q", "-b", "main"]).is_none() {
            eprintln!("git not installed: skipped");
            return;
        }
        run_git(&root, &["config", "core.autocrlf", "false"]).unwrap();
        let git_branches = || -> Vec<String> {
            run_git(
                &root,
                &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
            )
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
        };
        let git_stashes = || -> Vec<(String, String, String)> {
            run_git(&root, &["stash", "list", "--format=%H%x00%gd%x00%gs"])
                .unwrap()
                .lines()
                .map(|line| {
                    let parts: Vec<_> = line.split('\0').collect();
                    (parts[0].into(), parts[1].into(), parts[2].into())
                })
                .collect()
        };
        let same = |why: &str| {
            assert_eq!(branches(&root), Some(git_branches()), "{why}: branches");
            assert_eq!(stashes(&root), Some(git_stashes()), "{why}: stashes");
        };
        same("no commit yet");
        std::fs::write(root.join("f"), "1").unwrap();
        run_git(&root, &["add", "f"]).unwrap();
        run_git(&root, &["commit", "-q", "-m", "one"]).unwrap();
        let mut names = vec!["b", "a/z", "a-z", "Zed", "a.b"];
        // On macOS names that are not ASCII are left to Git (see `branches`).
        if !cfg!(target_os = "macos") {
            names.push("ž/č");
        }
        for branch in names {
            run_git(&root, &["branch", branch]).unwrap();
        }
        same("loose branches, sorted as Git sorts them");
        for content in ["2", "3", "4"] {
            std::fs::write(root.join("f"), content).unwrap();
            run_git(
                &root,
                &["stash", "push", "-q", "-m", &format!("edit {content}")],
            )
            .unwrap();
        }
        same("three stashes");
        run_git(&root, &["stash", "drop", "-q", "stash@{1}"]).unwrap();
        same("a stash dropped from the middle");
        run_git(&root, &["pack-refs", "--all"]).unwrap();
        run_git(&root, &["branch", "c"]).unwrap();
        same("packed and loose branches");
        // Another reference answering to a branch's short name makes Git print `heads/<b>`.
        run_git(&root, &["tag", "b"]).unwrap();
        assert_eq!(branches(&root), None, "ambiguous short name");
        run_git(&root, &["tag", "-d", "b"]).unwrap();
        // Git's loose lookup ignores case where the file system does.
        if cfg!(any(windows, target_os = "macos")) {
            run_git(&root, &["update-ref", "refs/tags/B", "HEAD"]).unwrap();
            assert_eq!(branches(&root), None, "short name taken in another case");
            run_git(&root, &["update-ref", "-d", "refs/tags/B"]).unwrap();
        }
        run_git(
            &root,
            &["symbolic-ref", "refs/heads/alias", "refs/heads/main"],
        )
        .unwrap();
        assert_eq!(branches(&root), None, "symbolic branch");
        // HEAD on that branch: Git names the branch at the end of the chain.
        run_git(&root, &["symbolic-ref", "HEAD", "refs/heads/alias"]).unwrap();
        assert_eq!(head(&root), None, "HEAD on a symbolic branch");
        run_git(&root, &["symbolic-ref", "HEAD", "refs/heads/main"]).unwrap();
        run_git(&root, &["symbolic-ref", "--delete", "refs/heads/alias"]).unwrap();
        while run_git(&root, &["stash", "drop", "-q"]).is_some() {}
        same("every stash dropped");
    }

    #[test]
    fn reftable_repositories_are_left_to_git() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        if run_git(&root, &["init", "-q", "--ref-format=reftable"]).is_none() {
            eprintln!("git without reftable: skipped");
            return;
        }
        run_git(&root, &["commit", "-q", "--allow-empty", "-m", "one"]).unwrap();
        assert!(repository_files(&root).is_none());
        assert_eq!(head(&root), None);
        assert_eq!(detached_head(&root), None);
    }

    #[test]
    fn reference_names_are_accepted_exactly_when_git_accepts_them() {
        let t = tempfile::tempdir().unwrap();
        if run_git(t.path(), &["--version"]).is_none() {
            eprintln!("git not installed: skipped");
            return;
        }
        let names = [
            "main",
            "feature/my-change",
            "a.b",
            "a/b.c/d",
            "ž/č",
            "-dash",
            "x-",
            "a_b+c=d,e%f",
            "HEAD",
            "@",
            "a@b",
            "a@{b",
            "a..b",
            "a.",
            ".a",
            "a/.b",
            "a.lock",
            "a.lock/b",
            "a/b.lock",
            "a/",
            "a//b",
            "/a",
            "a b",
            "a\tb",
            "a\u{7f}b",
            "a~b",
            "a^b",
            "a:b",
            "a?b",
            "a*b",
            "a[b",
            "a]b",
            "a\\b",
            "a{b}",
            "a!b",
            "a'b\"c",
            "a.lockb",
            "...",
            "",
        ];
        for prefix in ["refs/heads/", "refs/tags/"] {
            for name in names {
                let full = format!("{prefix}{name}");
                let git = run_git(t.path(), &["check-ref-format", &full]).is_some();
                assert_eq!(plain_ref_name(&full), git, "{full:?}");
            }
        }
        for name in ["origin", "a.b", "a/b", "a..b", ".x", "x.lock"] {
            let full = format!("refs/remotes/{name}/check");
            let git = run_git(t.path(), &["check-ref-format", &full]).is_some();
            assert_eq!(plain_ref_name(&full), git, "{full:?}");
        }
    }

    #[test]
    fn markers_from_one_listing_answer_as_looking_them_up_does() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        if run_git(&root, &["init", "-q", "-b", "main"]).is_none() {
            eprintln!("git not installed: skipped");
            return;
        }
        let dir = git_dir(&root).unwrap();
        std::fs::write(dir.join("MERGE_HEAD"), "x\n").unwrap();
        std::fs::create_dir(dir.join("rebase-merge")).unwrap();
        std::fs::write(dir.join("rebase-merge").join("done"), "edit 1\n").unwrap();
        let listed = markers(&root).expect("the Git directory is listed");
        for name in [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "rebase-merge",
            "rebase-merge/done",
            "rebase-merge/amend",
            "rebase-apply/next",
            "sequencer/todo",
        ] {
            let path = dir.join(name);
            assert_eq!(listed.exists(name), path.exists(), "{name}");
            assert_eq!(listed.read(name), std::fs::read(&path).ok(), "{name}");
        }
        // Not a work tree root: Git is asked instead.
        assert!(markers(&t.path().join("missing")).is_none());
    }

    #[test]
    fn a_detached_head_read_from_its_file_matches_git() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        if run_git(&root, &["init", "-q", "-b", "main"]).is_none() {
            eprintln!("git not installed: skipped");
            return;
        }
        // What `retain_detached` asked Git: `symbolic-ref -q HEAD`, then the commit.
        let git_detached = |dir: &Path| -> Option<String> {
            match run_git(dir, &["symbolic-ref", "-q", "HEAD"]) {
                Some(_) => None,
                None => Some(
                    run_git(dir, &["rev-parse", "--verify", "HEAD"])?
                        .trim()
                        .into(),
                ),
            }
        };
        assert_eq!(detached_head(&root), Some(None), "unborn branch");
        assert_eq!(git_detached(&root), None);
        run_git(&root, &["commit", "-q", "--allow-empty", "-m", "one"]).unwrap();
        assert_eq!(
            detached_head(&root),
            Some(git_detached(&root)),
            "on a branch"
        );
        run_git(&root, &["switch", "-q", "--detach"]).unwrap();
        let detached = detached_head(&root);
        assert!(matches!(detached, Some(Some(_))), "{detached:?}");
        assert_eq!(detached, Some(git_detached(&root)), "detached");
        let linked = t.path().join("linked");
        let linked_arg = linked.to_string_lossy().into_owned();
        run_git(&root, &["worktree", "add", "-q", "-b", "side", &linked_arg]).unwrap();
        assert_eq!(
            detached_head(&linked),
            Some(None),
            "linked worktree on a branch"
        );
        assert_eq!(git_detached(&linked), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_root_reached_through_a_link_follows_the_link() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        for repo in [&a, &b] {
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            std::fs::create_dir(repo.join("src")).unwrap();
        }
        let link = t.path().join("current");
        std::os::unix::fs::symlink(&a, &link).unwrap();
        let dir = link.join("src");
        assert_eq!(root(&dir, || Some(a.clone())), Some(a.clone()));
        assert_eq!(root(&dir, || panic!("kept")), Some(a.clone()));
        // The link now leads to another repository: the kept root is not it.
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&b, &link).unwrap();
        assert_eq!(root(&dir, || Some(b.clone())), Some(b));
    }

    #[test]
    fn the_root_is_asked_once_and_again_when_the_work_tree_changes() {
        let t = tempfile::tempdir().unwrap();
        let outer = t.path().join("outer");
        let inner = outer.join("inner");
        std::fs::create_dir_all(inner.join("src")).unwrap();
        let dir = inner.join("src");
        assert_eq!(
            root(&dir, || panic!("no repository: Git is not asked")),
            None
        );
        std::fs::create_dir(outer.join(".git")).unwrap();
        assert_eq!(root(&dir, || Some(outer.clone())), Some(outer.clone()));
        assert_eq!(root(&dir, || panic!("kept")), Some(outer.clone()));
        // A repository appears in between: the kept answer no longer applies.
        std::fs::create_dir(inner.join(".git")).unwrap();
        assert_eq!(root(&dir, || Some(inner.clone())), Some(inner.clone()));
        assert_eq!(root(&dir, || None), Some(inner));
    }
}
