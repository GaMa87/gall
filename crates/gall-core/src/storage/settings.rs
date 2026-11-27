//! User settings (C-01). Every field has a default so older/newer files load.

use crate::nav::path::PathFormat;
use crate::nav::session::NewTabPlacement;
use crate::view::sort::{GroupMode, GroupOrder, SortKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ThemeMode {
    #[default]
    System,
    Dark,
    Light,
    Omarchy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum WindowButtonsMode {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceBarMode {
    /// Always show the numbered workspace switcher (`1 2 3 +`) in the title bar.
    #[default]
    Always,
    /// Show the switcher only when more than one workspace is open.
    Multiple,
    /// Hide the workspace switcher from the title bar.
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DragBehavior {
    /// Drop moves (Ctrl copies, Alt asks), regardless of drive. (O-11 default)
    #[default]
    Consistent,
    /// Drop copies (Shift moves, Alt asks).
    Copy,
    /// Same drive moves, other drive copies.
    System,
    /// Always show the menu.
    Ask,
}

/// "Always open … with …", made with Ctrl+Enter in the Open with palette.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRule {
    /// What it applies to: `ext:md` (lower-case extension) or `path:<file>` (one file).
    pub target: String,
    /// Only for files inside this folder (a project or any folder); none = everywhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// Display name of the application.
    pub name: String,
    /// What to run (see `util::openwith::AppChoice::command`).
    pub command: String,
}

impl OpenRule {
    /// The rule for a file: one made for that file, else the one for its extension in
    /// the deepest folder containing it, else the one for its extension everywhere.
    pub fn find<'a>(rules: &'a [OpenRule], path: &std::path::Path) -> Option<&'a OpenRule> {
        // Windows paths are case-insensitive: compare them lower-cased there.
        let fold = |s: &str| {
            if cfg!(windows) {
                s.to_lowercase()
            } else {
                s.to_string()
            }
        };
        let path = std::path::PathBuf::from(fold(&path.to_string_lossy()));
        let exact = format!("path:{}", path.to_string_lossy());
        if let Some(r) = rules.iter().find(|r| fold(&r.target) == exact) {
            return Some(r);
        }
        let ext = path.extension()?.to_string_lossy().to_lowercase();
        let target = format!("ext:{ext}");
        rules
            .iter()
            .filter(|r| r.target == target)
            .filter(|r| r.dir.as_deref().is_none_or(|d| path.starts_with(fold(d))))
            // Deeper folder = more specific; "everywhere" counts as depth 0.
            .max_by_key(|r| {
                r.dir
                    .as_deref()
                    .map_or(0, |d| std::path::Path::new(d).components().count() + 1)
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum VideoSeek {
    Seconds,
    #[default]
    Fraction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyboardProfile {
    Explorer,
    TotalCommander,
    Vim,
}

/// One command the user defined for the Git workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct GitCommandDef {
    pub id: String,
    pub label: String,
    /// Where it applies: `commit`, `branch`, `tag`, `file` (Working Changes) or `stash`.
    pub context: String,
    /// The program and its arguments, with `{hash}`, `{branch}`, `{file}`, `{selection}`, `{repo}`, `{tag}`, `{stash}`.
    pub command: Vec<String>,
    /// One key in the panel of its context (with Vim keys); empty: none.
    pub key: String,
    /// Ask before running.
    pub confirm: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    // Appearance (A-01…A-06)
    pub theme: ThemeMode,
    /// Follow Omarchy's `window-glassy.lua` toggle when running on Omarchy.
    pub omarchy_glassy: bool,
    /// Follow Omarchy's `fontconfig` monospace font when the Omarchy theme is active.
    pub omarchy_follow_font: bool,
    /// Name of a theme file in the themes folder; empty = built-in.
    pub custom_theme: String,
    pub accent: String,
    pub font_family: String,
    /// "regular", "bold", "italic" or "boldItalic": the text of the whole window.
    pub font_style: String,
    pub font_size: f32,
    pub row_padding: f32,
    pub thin_scrollbars: bool,
    pub animations: bool,
    pub age_saturation: f32,
    pub age_lightness: f32,
    pub folder_background_images: bool,

    // View (F-03, F-08)
    pub show_hidden: bool,
    pub hide_temp: bool,
    /// Before `group` existed; only read to move that choice over (`migrate_defaults`).
    pub folders_on_top: bool,
    pub folders_by_name: bool,
    /// What is collected before the sort orders each group.
    pub group: GroupMode,
    pub group_order: GroupOrder,
    /// Grouping by type: images, documents, … instead of each extension.
    pub group_by_kind: bool,
    /// Headers above the groups; off: a flat list in the same order.
    pub group_headers: bool,
    pub default_sort: SortKey,
    pub default_descending: bool,
    pub default_view: String,
    /// Sort and view changed in a folder are kept for that folder (view for the folders
    /// below too). Off: every folder uses the defaults above, and changing the sort or the
    /// view in a folder changes those defaults; what was kept stays for when it is on again.
    pub remember_folder_views: bool,
    pub marching_ants: bool,
    /// Git status badges in the file list and the branch in the status bar.
    pub show_git_status: bool,
    /// Where a new worktree goes: `{repo}` (folder of the main worktree, elsewhere in the
    /// template its name) and `{branch}`. Empty = `{repo}/.worktrees/{branch}`.
    pub worktree_location: String,
    /// Double-click on a commit: "details" (its changes) or "checkout" (asks first).
    pub git_double_click: String,
    /// Conflict forecast in the branch list: not in repositories with more branches; 0 is off.
    pub git_forecast_branches: u32,
    /// Main worktree folder → its own template, instead of `worktree_location`.
    pub worktree_locations: BTreeMap<String, String>,
    /// Switching with local changes keeps them with their branch and brings them back on return (`fs::gitwip`). Off by default.
    pub git_branch_wip: bool,
    /// Commands of the user for the Git workspace (arguments, never a shell string), with the variables of `fs::gitcustom`.
    pub git_commands: Vec<GitCommandDef>,
    /// Main worktree folder → the command (program and arguments) run in a new worktree after it was created.
    pub worktree_commands: BTreeMap<String, Vec<String>>,
    /// Main worktree folder → link the folders of `.worktreeinclude` into new worktrees instead of copying them.
    pub worktree_links: BTreeMap<String, bool>,
    /// Calculate the size of every folder in the open folder (else only on request).
    pub folder_sizes_auto: bool,
    /// Preview + properties pane next to the file list (P-01). Off by default.
    pub show_details_pane: bool,
    /// Miller columns above the file list (true) or a breadcrumb bar (false), for
    /// browsers that were not switched themselves (` / leader w c).
    pub show_nav_columns: bool,

    // Columns (N-03)
    pub column_hover_expand: bool,
    /// Inside the home folder, Miller columns start at home instead of `/`.
    pub columns_from_home: bool,
    pub column_expand_delay_ms: u32,
    pub column_expand_duration_ms: u32,
    pub column_min_width: u32,
    pub column_max_width: u32,

    // Behaviour (M:other)
    pub single_click_open: bool,
    pub drag_behavior: DragBehavior,
    pub slow_click_rename: bool,
    pub startup_paths: Vec<String>,
    pub new_tab_path: String,
    pub new_tab_placement: NewTabPlacement,
    pub tabs_show_full_path: bool,
    /// When the workspace switcher (`1 2 3 +`) appears in the title bar.
    pub show_workspaces: WorkspaceBarMode,
    /// Show Live Tab Pairs bridge controls (`PanelMap` + `TabBar` pair actions).
    pub live_tab_pairs: bool,
    /// Show zero-config Pair Gravity ghost tab suggestions (`🧲`) on the opposite TabBar.
    pub pair_gravity: bool,

    /// Interface language: `auto` (Omarchy's language, else the system's) or a language code such as `sl`.
    pub language: String,

    // Window / sidebar (M:window)
    /// Open the main window maximized on start-up.
    pub start_maximized: bool,
    /// Automatically adapt pane split orientation (horizontal / vertical / single) to window size.
    pub adaptive_layout: bool,
    /// The sidebar is shown (the user's choice; a narrow window hides it temporarily without
    /// changing this).
    pub sidebar_visible: bool,
    /// Titlebar window control buttons visibility (`auto` hides on tiling WMs).
    pub window_buttons: WindowButtonsMode,
    pub show_drives: bool,
    /// Sidebar section with the cloud folders found on this computer (OneDrive, …).
    pub show_cloud: bool,
    pub show_history: bool,
    /// Sidebar system monitor (CPU / GPU / memory, network, disk I/O).
    pub show_system_monitor: bool,
    pub history_count: u32,
    pub terminal: String,
    /// Text editor command for Edit (F4); empty = ask once in the Edit with palette.
    pub editor: String,
    /// Applications chosen for file types (Open with › Ctrl+Enter).
    pub open_rules: Vec<OpenRule>,
    pub notification_ms: u32,
    pub path_copy_format: PathFormat,
    pub recent_destinations: u32,

    // Preview (M:preview1)
    pub preview_text_limit: usize,
    pub autoplay_media: bool,
    pub video_seek: VideoSeek,
    pub video_seek_seconds: u32,
    pub video_seek_fraction: u32,
    pub maps_url: String,
    pub no_thumbnail_exts: Vec<String>,
    pub excerpt_exts: Vec<String>,
    pub excerpt_lines: u32,

    // Search (Quick Open index and find in files)
    /// Folders indexed for Quick Open; empty = whole system (`/` or all drives).
    pub index_roots: Vec<String>,
    pub index_hidden: bool,
    pub index_respect_gitignore: bool,
    /// Folder/file names or globs that are never indexed or searched.
    pub index_exclude: Vec<String>,
    /// Background re-index interval; 0 = only on demand.
    pub index_refresh_minutes: u32,
    pub search_respect_gitignore: bool,

    // Missing in older settings: the UI derives the profile from vim_keys.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyboard_profile: Option<KeyboardProfile>,
    // Compatibility flag, kept in sync with the selected keyboard profile.
    pub vim_keys: bool,
    /// Leader key for multi-key sequences in vim mode: "Space", ";", "\\",
    /// "Ctrl" (tap) or "Ctrl Ctrl" (double tap).
    pub leader_key: String,
    /// Leader for the which-key menu when vim keys are off ("" = none).
    pub leader_key_basic: String,

    // Advanced
    pub watcher_debounce_ms: u32,
    /// Extra file names or glob patterns treated as generated (collapsed by default in comparisons and Git file lists).
    #[serde(default)]
    pub generated_patterns: Vec<String>,
    /// Download and verify updates automatically in the background, then prompt to restart.
    pub auto_download_updates: bool,
    /// Action name → key chord, overriding the built-in keymap (C-04).
    pub shortcuts: BTreeMap<String, String>,
    /// Folder path → icon colour (CSS colour), set from the folder context menu.
    pub folder_colors: BTreeMap<String, String>,
    /// Which changes of defaults this file has been through (`migrate_defaults`). A file
    /// written before this field existed reads as 0.
    #[serde(default)]
    pub defaults_version: u32,
}

/// The newest change of defaults `Settings::migrate_defaults` knows.
pub const DEFAULTS_VERSION: u32 = 3;

impl Settings {
    /// Moves values that still equal an old default to the current one, once per change:
    /// afterwards the old value is a choice like any other and is kept. Returns whether
    /// the settings changed and need saving.
    pub fn migrate_defaults(&mut self) -> bool {
        if self.defaults_version >= DEFAULTS_VERSION {
            return false;
        }
        if self.defaults_version < 1 {
            // 0.1.0 remembered 30 folders by default; the default is now 10.
            if self.history_count == 30 {
                self.history_count = 10;
            }
            // 0.1.2 and before squeezed Miller columns down to 70 px, too narrow for a name
            // (deep folders showed "C…" "Lo…"); now 120 px, and deeper paths scroll.
            if self.column_min_width == 70 {
                self.column_min_width = 120;
            }
        }
        if self.defaults_version < 2 {
            // Grouping by type is for new installs: an older one keeps the order it had.
            self.group = GroupMode::from_folders_on_top(self.folders_on_top);
        }
        if self.defaults_version < 3 && self.index_refresh_minutes == 15 {
            // 0.3.0 and before re-indexed every 15 minutes in the background; now on-demand
            // when Quick Open opens so idle sessions do zero background directory sweeps.
            self.index_refresh_minutes = 0;
        }
        self.defaults_version = DEFAULTS_VERSION;
        true
    }

    /// Expected path to Omarchy's active `colors.toml` theme file on Linux.
    #[cfg(target_os = "linux")]
    pub fn omarchy_colors_path() -> Option<std::path::PathBuf> {
        let state = std::env::var_os("XDG_STATE_HOME")
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(dirs::state_dir)
            .or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))?;
        Some(state.join("omarchy/current/theme/colors.toml"))
    }

    #[cfg(not(target_os = "linux"))]
    pub fn omarchy_colors_path() -> Option<std::path::PathBuf> {
        None
    }

    /// True if an Omarchy `colors.toml` exists at `colors_toml`.
    pub fn omarchy_theme_present_at(colors_toml: &std::path::Path) -> bool {
        colors_toml.is_file()
    }

    /// True on Linux when Omarchy's active `colors.toml` theme file exists.
    pub fn is_omarchy_environment() -> bool {
        #[cfg(target_os = "linux")]
        {
            Self::omarchy_colors_path().is_some_and(|p| Self::omarchy_theme_present_at(&p))
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    /// Applies first-run defaults (`ThemeMode::Omarchy` when `omarchy_present` is true
    /// and theme is still `ThemeMode::System`).
    pub fn apply_first_run_defaults_with(&mut self, omarchy_present: bool) {
        if omarchy_present && self.theme == ThemeMode::System {
            self.theme = ThemeMode::Omarchy;
        }
    }

    /// Applies first-run defaults for the current host environment.
    pub fn apply_first_run_defaults(&mut self) {
        self.apply_first_run_defaults_with(Self::is_omarchy_environment());
    }

    /// Loads `Settings` from `store`, applying first-run Omarchy defaults only when no
    /// primary or backup file existed (`LoadedFrom::Default`).
    pub fn load(
        store: &crate::storage::JsonStore,
    ) -> (Self, crate::storage::json_store::LoadedFrom) {
        let (mut settings, from) = store.load::<Self>();
        if from == crate::storage::json_store::LoadedFrom::Default {
            settings.apply_first_run_defaults();
        }
        (settings, from)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeMode::System,
            omarchy_glassy: true,
            omarchy_follow_font: true,
            custom_theme: String::new(),
            accent: "#3d8bfd".into(),
            font_family: String::new(),
            font_style: "regular".into(),
            font_size: 13.0,
            row_padding: 3.0,
            thin_scrollbars: true,
            animations: true,
            age_saturation: 0.55,
            age_lightness: 0.5,
            folder_background_images: true,

            show_hidden: false,
            hide_temp: false,
            folders_on_top: true,
            folders_by_name: false,
            group: GroupMode::default(),
            group_order: GroupOrder::default(),
            group_by_kind: false,
            group_headers: true,
            default_sort: SortKey::Modified,
            default_descending: true,
            default_view: "default".into(),
            remember_folder_views: true,
            marching_ants: true,
            show_git_status: true,
            worktree_location: crate::fs::gitlocation::DEFAULT_TEMPLATE.into(),
            git_double_click: "details".into(),
            git_forecast_branches: 50,
            worktree_locations: BTreeMap::new(),
            git_branch_wip: false,
            git_commands: Vec::new(),
            worktree_commands: BTreeMap::new(),
            worktree_links: BTreeMap::new(),
            folder_sizes_auto: false,
            show_details_pane: false,
            show_nav_columns: false,

            column_hover_expand: true,
            columns_from_home: true,
            column_expand_delay_ms: 400,
            column_expand_duration_ms: 150,
            column_min_width: 120,
            column_max_width: 260,

            single_click_open: false,
            drag_behavior: DragBehavior::Consistent,
            slow_click_rename: false,
            startup_paths: vec![],
            new_tab_path: String::new(),
            new_tab_placement: NewTabPlacement::AfterCurrent,
            tabs_show_full_path: false,
            show_workspaces: WorkspaceBarMode::Always,
            live_tab_pairs: true,
            pair_gravity: true,

            start_maximized: true,
            language: "auto".to_string(),
            adaptive_layout: true,
            sidebar_visible: true,
            window_buttons: WindowButtonsMode::Auto,
            show_drives: true,
            show_cloud: true,
            show_history: true,
            show_system_monitor: true,
            defaults_version: DEFAULTS_VERSION,
            history_count: 10,
            terminal: String::new(),
            editor: String::new(),
            open_rules: Vec::new(),
            notification_ms: 6000,
            path_copy_format: PathFormat::Native,
            recent_destinations: 32,

            preview_text_limit: 256 * 1024,
            autoplay_media: false,
            video_seek: VideoSeek::Fraction,
            video_seek_seconds: 10,
            video_seek_fraction: 8,
            maps_url: "https://www.openstreetmap.org/?mlat={lat}&mlon={lon}#map=16/{lat}/{lon}"
                .into(),
            no_thumbnail_exts: vec![],
            excerpt_exts: vec!["txt".into(), "md".into(), "2do".into(), "log".into()],
            excerpt_lines: 3,

            index_roots: vec![],
            index_hidden: false,
            index_respect_gitignore: true,
            index_exclude: crate::fs::index::default_excludes(),
            index_refresh_minutes: 0,
            search_respect_gitignore: true,

            keyboard_profile: None,
            vim_keys: true,
            leader_key: "Space".into(),
            leader_key_basic: "Ctrl Ctrl".into(),

            watcher_debounce_ms: 250,
            generated_patterns: Vec::new(),
            auto_download_updates: true,
            shortcuts: BTreeMap::new(),
            folder_colors: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod defaults_tests {
    use super::{Settings, DEFAULTS_VERSION};

    #[test]
    fn old_defaults_move_once_and_a_later_choice_is_kept() {
        // A file from before the field existed, still holding the old defaults.
        let mut s: Settings =
            serde_json::from_str(r#"{"historyCount":30,"columnMinWidth":70}"#).unwrap();
        assert_eq!(s.defaults_version, 0);
        assert!(s.migrate_defaults());
        assert_eq!((s.history_count, s.column_min_width), (10, 120));
        assert_eq!(s.defaults_version, DEFAULTS_VERSION);
        // The user now picks the old values on purpose: they survive a restart.
        s.history_count = 30;
        s.column_min_width = 70;
        let mut again: Settings =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(!again.migrate_defaults());
        assert_eq!((again.history_count, again.column_min_width), (30, 70));
    }

    #[test]
    fn folders_on_top_becomes_the_grouping_once() {
        use crate::view::sort::GroupMode;
        for (file, group) in [
            (
                r#"{"defaultsVersion":1,"foldersOnTop":true}"#,
                GroupMode::Folders,
            ),
            (
                r#"{"defaultsVersion":1,"foldersOnTop":false}"#,
                GroupMode::None,
            ),
            // The old default was on.
            (r#"{"historyCount":12}"#, GroupMode::Folders),
        ] {
            let mut s: Settings = serde_json::from_str(file).unwrap();
            assert!(s.migrate_defaults());
            assert_eq!(s.group, group, "{file}");
            // A choice of today is kept, whatever the old field says.
            s.group = GroupMode::Type;
            let mut again: Settings =
                serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
            assert!(!again.migrate_defaults());
            assert_eq!(again.group, GroupMode::Type);
        }
        assert_eq!(Settings::default().group, GroupMode::FoldersType);
        // Another change of defaults is not repeated on the way.
        let mut s: Settings =
            serde_json::from_str(r#"{"defaultsVersion":1,"historyCount":30}"#).unwrap();
        s.migrate_defaults();
        assert_eq!(s.history_count, 30);
    }

    #[test]
    fn fresh_settings_need_no_migration() {
        assert!(!Settings::default().migrate_defaults());
    }

    #[test]
    fn index_refresh_migrates_to_on_demand_once() {
        use crate::view::sort::GroupMode;
        let mut s: Settings = serde_json::from_str(
            r#"{"defaultsVersion":2,"group":"type","indexRefreshMinutes":15}"#,
        )
        .unwrap();
        assert!(s.migrate_defaults());
        assert_eq!(s.index_refresh_minutes, 0);
        // Existing v2 grouping choice must not be overwritten by the v3 migration.
        assert_eq!(s.group, GroupMode::Type);
        assert_eq!(s.defaults_version, DEFAULTS_VERSION);

        // Explicit custom interval or re-selecting 15 at v3 survives.
        let mut custom: Settings =
            serde_json::from_str(r#"{"defaultsVersion":2,"indexRefreshMinutes":30}"#).unwrap();
        assert!(custom.migrate_defaults());
        assert_eq!(custom.index_refresh_minutes, 30);

        s.index_refresh_minutes = 15;
        let mut again: Settings =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(!again.migrate_defaults());
        assert_eq!(again.index_refresh_minutes, 15);
    }
}

#[cfg(test)]
mod open_rule_tests {
    use super::OpenRule;
    use std::path::Path;

    fn r(target: &str, dir: Option<&str>, name: &str) -> OpenRule {
        OpenRule {
            target: target.into(),
            dir: dir.map(Into::into),
            name: name.into(),
            command: name.into(),
        }
    }

    #[test]
    fn most_specific_rule_wins() {
        let rules = vec![
            r("ext:md", None, "typora"),
            r("ext:md", Some("/home/a/Projects"), "zed"),
            r("ext:md", Some("/home/a/Projects/web"), "code"),
            r("path:/home/a/Projects/web/NOTES.md", None, "vim"),
            r("ext:txt", Some("/srv"), "kate"),
        ];
        let f = |p: &str| OpenRule::find(&rules, Path::new(p)).map(|r| r.name.as_str());
        assert_eq!(f("/home/a/notes.md"), Some("typora"));
        assert_eq!(f("/home/a/Projects/x/README.MD"), Some("zed"));
        assert_eq!(f("/home/a/Projects/web/a/b.md"), Some("code"));
        assert_eq!(f("/home/a/Projects/web/NOTES.md"), Some("vim"));
        assert_eq!(
            f("/home/a/Projects-old/a.md"),
            Some("typora"),
            "folder match is by path component"
        );
        assert_eq!(f("/home/a/x.txt"), None);
        assert_eq!(f("/srv/x.txt"), Some("kate"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_ignore_case() {
        let rules = vec![
            r("ext:md", Some(r"C:\Users\Ana\Projects"), "zed"),
            r(r"path:C:\Users\Ana\todo.md", None, "vim"),
        ];
        let f = |p: &str| OpenRule::find(&rules, Path::new(p)).map(|r| r.name.as_str());
        assert_eq!(f(r"c:\users\ana\projects\x\A.MD"), Some("zed"));
        assert_eq!(f(r"C:\USERS\ana\TODO.md"), Some("vim"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_file_fills_defaults() {
        let s: Settings =
            serde_json::from_str(r#"{"fontSize": 15, "unknownFutureKey": true}"#).unwrap();
        assert_eq!(s.font_size, 15.0);
        assert_eq!(s.notification_ms, 6000);
        assert!(s.auto_download_updates);

        let disabled: Settings = serde_json::from_str(r#"{"autoDownloadUpdates": false}"#).unwrap();
        assert!(!disabled.auto_download_updates);
    }

    #[test]
    fn legacy_keyboard_settings_remain_available_for_migration() {
        let s: Settings =
            serde_json::from_str(r#"{"vimKeys": false, "shortcuts": {"copyToOther": "F9"}}"#)
                .unwrap();
        assert_eq!(s.keyboard_profile, None);
        assert!(!s.vim_keys);
        assert_eq!(s.shortcuts.get("copyToOther").unwrap(), "F9");
    }

    #[test]
    fn keyboard_profiles_roundtrip_without_losing_custom_shortcuts() {
        for profile in [
            KeyboardProfile::Explorer,
            KeyboardProfile::TotalCommander,
            KeyboardProfile::Vim,
        ] {
            let s = Settings {
                keyboard_profile: Some(profile),
                vim_keys: profile == KeyboardProfile::Vim,
                shortcuts: BTreeMap::from([("copyToOther".into(), "F9".into())]),
                ..Settings::default()
            };
            let saved = serde_json::to_string(&s).unwrap();
            let restored: Settings = serde_json::from_str(&saved).unwrap();
            assert_eq!(restored, s);
        }
    }

    #[test]
    fn omarchy_theme_and_window_buttons_roundtrip_and_defaults() {
        let s = Settings {
            theme: ThemeMode::Omarchy,
            window_buttons: WindowButtonsMode::Never,
            adaptive_layout: false,
            omarchy_glassy: false,
            omarchy_follow_font: false,
            ..Settings::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""theme":"omarchy""#));
        assert!(json.contains(r#""windowButtons":"never""#));
        let restored: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, s);

        for (mode, expected) in [
            (WindowButtonsMode::Auto, r#""auto""#),
            (WindowButtonsMode::Always, r#""always""#),
            (WindowButtonsMode::Never, r#""never""#),
        ] {
            assert_eq!(serde_json::to_string(&mode).unwrap(), expected);
        }

        // Legacy partial JSON fills new fields with expected defaults and preserves theme.
        let legacy: Settings = serde_json::from_str(r#"{"theme":"system"}"#).unwrap();
        assert_eq!(legacy.theme, ThemeMode::System);
        assert!(legacy.adaptive_layout);
        assert_eq!(
            legacy.language, "auto",
            "an old settings file follows Omarchy / the system language"
        );
        assert!(legacy.sidebar_visible);
        let hidden: Settings = serde_json::from_str(r#"{"sidebarVisible":false}"#).unwrap();
        assert!(!hidden.sidebar_visible);
        assert_eq!(legacy.window_buttons, WindowButtonsMode::Auto);
        assert!(legacy.omarchy_glassy);
        assert!(legacy.omarchy_follow_font);
    }

    #[test]
    fn first_run_defaults_apply_omarchy_only_on_default_load() {
        let t = tempfile::tempdir().unwrap();
        let colors = t.path().join("colors.toml");
        assert!(!Settings::omarchy_theme_present_at(&colors));

        let mut s = Settings::default();
        assert_eq!(s.theme, ThemeMode::System);
        s.apply_first_run_defaults_with(Settings::omarchy_theme_present_at(&colors));
        assert_eq!(s.theme, ThemeMode::System);

        std::fs::write(&colors, "accent = \"#ff0000\"\n").unwrap();
        assert!(Settings::omarchy_theme_present_at(&colors));
        s.apply_first_run_defaults_with(Settings::omarchy_theme_present_at(&colors));
        assert_eq!(s.theme, ThemeMode::Omarchy);

        // Explicit user choice (Dark/Light) is never overridden by first-run helper.
        let mut dark = Settings {
            theme: ThemeMode::Dark,
            ..Settings::default()
        };
        dark.apply_first_run_defaults_with(true);
        assert_eq!(dark.theme, ThemeMode::Dark);

        // Existing settings.json loaded via JsonStore preserves ThemeMode::System.
        let store = crate::storage::JsonStore::new(t.path().join("settings.json"));
        store.save(&Settings::default()).unwrap();
        let (mut loaded, from) = store.load::<Settings>();
        assert_eq!(from, crate::storage::json_store::LoadedFrom::Primary);
        if from == crate::storage::json_store::LoadedFrom::Default {
            loaded.apply_first_run_defaults_with(true);
        }
        assert_eq!(loaded.theme, ThemeMode::System);
    }
}
