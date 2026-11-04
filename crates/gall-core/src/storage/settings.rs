//! User settings (C-01). Every field has a default so older/newer files load.

use crate::nav::path::PathFormat;
use crate::nav::session::NewTabPlacement;
use crate::view::sort::SortKey;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ThemeMode {
    #[default]
    System,
    Dark,
    Light,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    // Appearance (A-01…A-06)
    pub theme: ThemeMode,
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
    pub folders_on_top: bool,
    pub folders_by_name: bool,
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

    // Window / sidebar (M:window)
    /// Open the main window maximized on start-up.
    pub start_maximized: bool,
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

    // Vim-style keys (j/k, gg/G, leader sequences)
    pub vim_keys: bool,
    /// Leader key for multi-key sequences in vim mode: "Space", ";", "\\",
    /// "Ctrl" (tap) or "Ctrl Ctrl" (double tap).
    pub leader_key: String,
    /// Leader for the which-key menu when vim keys are off ("" = none).
    pub leader_key_basic: String,

    // Advanced
    pub watcher_debounce_ms: u32,
    /// Action name → key chord, overriding the built-in keymap (C-04).
    pub shortcuts: BTreeMap<String, String>,
    /// Folder path → icon colour (CSS colour), set from the folder context menu.
    pub folder_colors: BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeMode::System,
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
            default_sort: SortKey::Name,
            default_descending: false,
            default_view: "default".into(),
            remember_folder_views: true,
            marching_ants: true,
            show_git_status: true,
            folder_sizes_auto: false,
            show_details_pane: false,
            show_nav_columns: true,

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

            start_maximized: true,
            show_drives: true,
            show_cloud: true,
            show_history: true,
            show_system_monitor: true,
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
            index_refresh_minutes: 15,
            search_respect_gitignore: true,

            vim_keys: true,
            leader_key: "Space".into(),
            leader_key_basic: "Ctrl Ctrl".into(),

            watcher_debounce_ms: 250,
            shortcuts: BTreeMap::new(),
            folder_colors: BTreeMap::new(),
        }
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
    }
}
