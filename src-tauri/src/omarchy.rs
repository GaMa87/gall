//! Live Omarchy desktop bridge (theme colors, custom CSS, Hyprland rounding,
//! fontconfig monospace font, glassy toggle, and tiling-WM detection).
//!
//! Zero-overhead on non-Linux hosts and on Linux hosts without Omarchy/Hyprland.

use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

/// The Omarchy interface language (`~/.config/omarchy/language`, described by Omarchy's `locales.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OmarchyLanguage {
    /// Omarchy language code, e.g. `sl`, `pt-BR`.
    pub code: String,
    /// BCP 47 tag for number, date and size formatting, e.g. `sl-SI`.
    pub format_locale: String,
    /// `ltr` or `rtl`.
    pub direction: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OmarchyState {
    pub active: bool,
    pub theme_mode: String,
    pub palette_tokens: BTreeMap<String, String>,
    pub custom_css: Option<String>,
    pub rounding: Option<u32>,
    pub glassy: bool,
    pub monospace_font: Option<String>,
    pub tiling_wm: bool,
    #[serde(default)]
    pub floating_workspace: bool,
    pub language: Option<OmarchyLanguage>,
}

impl Default for OmarchyState {
    fn default() -> Self {
        Self {
            active: false,
            theme_mode: "dark".to_string(),
            palette_tokens: BTreeMap::new(),
            custom_css: None,
            rounding: None,
            glassy: false,
            monospace_font: None,
            tiling_wm: false,
            floating_workspace: false,
            language: None,
        }
    }
}

/// The language named by the first line of `language_file`, looked up in `locales_json`.
/// Unknown or missing codes mean English, as in `omarchy language`.
#[allow(dead_code)]
pub fn read_language(
    language_file: &std::path::Path,
    locales_json: &std::path::Path,
) -> OmarchyLanguage {
    let english = OmarchyLanguage {
        code: "en".into(),
        format_locale: "en-US".into(),
        direction: "ltr".into(),
    };
    let code = std::fs::read_to_string(language_file)
        .ok()
        .and_then(|t| t.lines().next().map(|l| l.trim().to_string()))
        .filter(|c| !c.is_empty());
    let (Some(code), Ok(text)) = (code, std::fs::read_to_string(locales_json)) else {
        return english;
    };
    let Ok(all) = serde_json::from_str::<serde_json::Value>(&text) else {
        return english;
    };
    let Some(entry) = all.get(&code) else {
        return english;
    };
    OmarchyLanguage {
        format_locale: entry
            .get("formatLocale")
            .and_then(|v| v.as_str())
            .unwrap_or("en-US")
            .to_string(),
        direction: if entry.get("direction").and_then(|v| v.as_str()) == Some("rtl") {
            "rtl".into()
        } else {
            "ltr".into()
        },
        code,
    }
}

/// Omarchy's install folder (`$OMARCHY_PATH`, else `/usr/share/omarchy`).
#[allow(dead_code)]
pub fn omarchy_path() -> std::path::PathBuf {
    std::env::var_os("OMARCHY_PATH")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/usr/share/omarchy"))
}

#[derive(Clone)]
pub struct OmarchyBridge {
    state: Arc<RwLock<OmarchyState>>,
}

impl OmarchyBridge {
    pub fn new_inactive() -> Self {
        Self {
            state: Arc::new(RwLock::new(OmarchyState {
                tiling_wm: is_tiling_wm(),
                ..OmarchyState::default()
            })),
        }
    }

    pub fn current(&self) -> OmarchyState {
        self.state.read().unwrap().clone()
    }

    /// Triggers a short delayed Hyprland state refresh (e.g. right after a window maps so
    /// Omarchy's floating-workspace tag is picked up promptly).
    pub fn refresh_hyprland_async(&self, app: &tauri::AppHandle) {
        #[cfg(target_os = "linux")]
        {
            linux::refresh_hyprland_delayed(app.clone(), self.state.clone());
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = app;
        }
    }

    /// Starts the Omarchy live bridge if running on Linux in an Omarchy/Hyprland session.
    /// Otherwise returns an inactive bridge with zero background threads, sockets, or subprocesses.
    pub fn start(app: tauri::AppHandle) -> Self {
        #[cfg(target_os = "linux")]
        {
            linux::start_bridge(app)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = app;
            Self::new_inactive()
        }
    }
}

/// Known tiling window manager / compositor names matched against `XDG_CURRENT_DESKTOP`
/// and `XDG_SESSION_DESKTOP`.
#[cfg(any(target_os = "linux", test))]
const TILING_WMS: &[&str] = &[
    "hyprland",
    "sway",
    "niri",
    "i3",
    "bspwm",
    "awesome",
    "dwm",
    "river",
    "qtile",
    "xmonad",
    "herbstluftwm",
    "leftwm",
];

/// Pure tiling-WM check parameterized by environment lookup for cross-platform testing.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn is_tiling_wm_with<F: Fn(&str) -> Option<String>>(get_env: F) -> bool {
    for var in [
        "HYPRLAND_INSTANCE_SIGNATURE",
        "SWAYSOCK",
        "NIRI_SOCKET",
        "I3SOCK",
    ] {
        if get_env(var).is_some_and(|v| !v.trim().is_empty()) {
            return true;
        }
    }
    for var in ["XDG_CURRENT_DESKTOP", "XDG_SESSION_DESKTOP"] {
        if let Some(val) = get_env(var) {
            for part in val.split(':') {
                let norm = part.trim().to_ascii_lowercase();
                if TILING_WMS.contains(&norm.as_str()) {
                    return true;
                }
            }
        }
    }
    false
}

/// True on Linux when running under Hyprland, Sway, Niri, i3, or another known tiling WM.
/// Always false on Windows and macOS.
pub fn is_tiling_wm() -> bool {
    #[cfg(target_os = "linux")]
    {
        is_tiling_wm_with(|k| std::env::var(k).ok())
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use notify_debouncer_full::notify::{
        event::{AccessKind, AccessMode},
        Config as NotifyConfig, Event, EventKind, RecommendedWatcher, RecursiveMode,
    };
    use notify_debouncer_full::{new_debouncer_opt, DebounceEventResult, Debouncer, NoCache};
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixStream;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    use tauri::Emitter;

    const MAX_CUSTOM_CSS_BYTES: u64 = 256 * 1024;
    const HYPRCTL_TIMEOUT: Duration = Duration::from_millis(500);
    const WATCH_DEBOUNCE: Duration = Duration::from_millis(80);

    #[derive(Clone, Debug)]
    pub(super) struct OmarchyPaths {
        pub(super) current_dir: PathBuf,
        pub(super) theme_dir: PathBuf,
        pub(super) colors_toml: PathBuf,
        pub(super) light_mode_marker: PathBuf,
        pub(super) gall_css: PathBuf,
        pub(super) fonts_dir: PathBuf,
        pub(super) fonts_conf: PathBuf,
        pub(super) glassy_dir: PathBuf,
        pub(super) glassy_lua: PathBuf,
        pub(super) omarchy_config_dir: PathBuf,
        pub(super) language_file: PathBuf,
    }

    impl OmarchyPaths {
        pub(super) fn from_env() -> Option<Self> {
            let state_base = std::env::var_os("XDG_STATE_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .or_else(dirs::state_dir)
                .or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))?;
            let config_base = std::env::var_os("XDG_CONFIG_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .or_else(dirs::config_dir)
                .or_else(|| dirs::home_dir().map(|h| h.join(".config")))?;
            Some(Self::from_bases(&state_base, &config_base))
        }

        pub(super) fn from_bases(state_base: &Path, config_base: &Path) -> Self {
            let current_dir = state_base.join("omarchy/current");
            let theme_dir = current_dir.join("theme");
            let colors_toml = theme_dir.join("colors.toml");
            let light_mode_marker = theme_dir.join("light.mode");
            let gall_css = theme_dir.join("gall.css");
            let fonts_dir = config_base.join("fontconfig");
            let fonts_conf = fonts_dir.join("fonts.conf");
            let glassy_dir = state_base.join("omarchy/toggles/hypr");
            let glassy_lua = glassy_dir.join("window-glassy.lua");
            let omarchy_config_dir = config_base.join("omarchy");
            let language_file = omarchy_config_dir.join("language");
            Self {
                current_dir,
                theme_dir,
                colors_toml,
                light_mode_marker,
                gall_css,
                fonts_dir,
                fonts_conf,
                glassy_dir,
                glassy_lua,
                omarchy_config_dir,
                language_file,
            }
        }
    }

    fn has_hyprland_env() -> bool {
        std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some_and(|v| !v.is_empty())
    }

    fn is_omarchy_active_env(paths: &OmarchyPaths) -> bool {
        paths.current_dir.exists() || has_hyprland_env()
    }

    pub(super) fn start_bridge(app: tauri::AppHandle) -> OmarchyBridge {
        let Some(paths) = OmarchyPaths::from_env() else {
            return OmarchyBridge::new_inactive();
        };
        if !is_omarchy_active_env(&paths) {
            return OmarchyBridge::new_inactive();
        }

        if has_hyprland_env() {
            suppress_hyprbars_for_gall();
        }
        let initial = compute_state(&paths, true);
        let shared = Arc::new(RwLock::new(initial));
        let bridge = OmarchyBridge {
            state: shared.clone(),
        };

        spawn_fs_watcher(app.clone(), paths.clone(), shared.clone());
        if has_hyprland_env() {
            spawn_hyprland_socket_listener(app, paths, shared);
        }

        bridge
    }

    pub(super) fn refresh_hyprland_delayed(
        app: tauri::AppHandle,
        shared: Arc<RwLock<OmarchyState>>,
    ) {
        if !has_hyprland_env() {
            return;
        }
        let Some(paths) = OmarchyPaths::from_env() else {
            return;
        };
        if !is_omarchy_active_env(&paths) {
            return;
        }
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            suppress_hyprbars_for_gall();
            refresh_and_emit(&app, &paths, &shared, true);
        });
    }

    /// Serializes refreshes: the file watcher and the Hyprland listener both recompute the state, and a
    /// slow computation that started earlier must not overwrite the result of a newer one.
    static REFRESH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn refresh_and_emit(
        app: &tauri::AppHandle,
        paths: &OmarchyPaths,
        shared: &Arc<RwLock<OmarchyState>>,
        query_hypr: bool,
    ) {
        let _serial = REFRESH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = compute_state(paths, query_hypr);
        if !query_hypr {
            // Preserve last known Hyprland fields when triggered purely by a non-Hyprland file edit.
            let prev = shared.read().unwrap();
            if prev.rounding.is_some() {
                next.rounding = prev.rounding;
            }
            next.floating_workspace = prev.floating_workspace;
        }
        let changed = {
            let mut guard = shared.write().unwrap();
            if *guard != next {
                *guard = next.clone();
                true
            } else {
                false
            }
        };
        if changed {
            let _ = app.emit("omarchy-state-changed", &next);
        }
    }

    fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
        let mut cur = Some(path);
        while let Some(p) = cur {
            if p.exists() {
                return Some(p.to_path_buf());
            }
            cur = p.parent();
        }
        None
    }

    fn arm_watches(
        debouncer: &mut Debouncer<RecommendedWatcher, NoCache>,
        paths: &OmarchyPaths,
        watched: &mut Vec<PathBuf>,
    ) {
        for prev in watched.drain(..) {
            let _ = debouncer.unwatch(&prev);
        }
        let candidates = [
            paths.current_dir.as_path(),
            paths.theme_dir.as_path(),
            paths.colors_toml.as_path(),
            paths.gall_css.as_path(),
            paths.fonts_dir.as_path(),
            paths.fonts_conf.as_path(),
            paths.glassy_dir.as_path(),
            paths.glassy_lua.as_path(),
            paths.omarchy_config_dir.as_path(),
            paths.language_file.as_path(),
        ];
        for cand in candidates {
            let target = if cand.exists() {
                Some(cand.to_path_buf())
            } else {
                nearest_existing_ancestor(cand)
            };
            if let Some(p) = target {
                if !watched.contains(&p) && debouncer.watch(&p, RecursiveMode::NonRecursive).is_ok()
                {
                    watched.push(p);
                }
            }
        }
    }

    /// Ignore read-only inotify access events (`Open`, `Close(NoWrite)`, `Read`):
    /// reading `colors.toml` / `fonts.conf` inside `refresh_and_emit` would otherwise
    /// trigger another watch batch every 80 ms in an infinite loop.
    pub(super) fn is_mutation_event(event: &Event) -> bool {
        event.need_rescan()
            || match event.kind {
                EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
                EventKind::Access(_) => false,
                _ => true,
            }
    }

    fn spawn_fs_watcher(
        app: tauri::AppHandle,
        paths: OmarchyPaths,
        shared: Arc<RwLock<OmarchyState>>,
    ) {
        let _ = std::thread::Builder::new()
            .name("omarchy-watch".into())
            .spawn(move || {
                let (tx, rx) = std::sync::mpsc::channel::<DebounceEventResult>();
                let Ok(mut debouncer) = new_debouncer_opt::<_, RecommendedWatcher, NoCache>(
                    WATCH_DEBOUNCE,
                    None,
                    tx,
                    NoCache::new(),
                    NotifyConfig::default(),
                ) else {
                    log::warn!("omarchy: could not initialize file watcher");
                    return;
                };
                let mut watched = Vec::new();
                arm_watches(&mut debouncer, &paths, &mut watched);

                while let Ok(res) = rx.recv() {
                    if let Ok(events) = res {
                        if !events.iter().any(|ev| is_mutation_event(ev)) {
                            continue;
                        }
                        // Re-arm watches in case `current/theme` was replaced atomically (`rm -rf && mv`).
                        arm_watches(&mut debouncer, &paths, &mut watched);
                        refresh_and_emit(&app, &paths, &shared, false);
                    }
                }
            });
    }

    fn hyprland_socket2_path() -> Option<PathBuf> {
        let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
            .ok()
            .filter(|s| !s.trim().is_empty())?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(dirs::runtime_dir)?;
        Some(runtime.join("hypr").join(sig).join(".socket2.sock"))
    }

    fn spawn_hyprland_socket_listener(
        app: tauri::AppHandle,
        paths: OmarchyPaths,
        shared: Arc<RwLock<OmarchyState>>,
    ) {
        let _ = std::thread::Builder::new()
            .name("omarchy-hypr-sock".into())
            .spawn(move || {
                let mut backoff_ms = 250u64;
                loop {
                    let Some(sock_path) = hyprland_socket2_path() else {
                        return;
                    };
                    if !sock_path.exists() {
                        std::thread::sleep(Duration::from_millis(backoff_ms));
                        // Keep trying at a capped pace: the compositor may come back (restart).
                        backoff_ms = (backoff_ms * 2).min(5_000);
                        continue;
                    }
                    match UnixStream::connect(&sock_path) {
                        Ok(stream) => {
                            let mut received_any = false;
                            let reader = BufReader::new(stream);
                            for line in reader.lines() {
                                match line {
                                    Ok(ev) => {
                                        received_any = true;
                                        backoff_ms = 250;
                                        if ev.starts_with("configreloaded>>")
                                            || ev.starts_with("changefloatingmode>>")
                                            || ev.starts_with("movewindowv2>>")
                                            || ev.starts_with("workspacev2>>")
                                            || (ev.starts_with("openwindow>>")
                                                && ev.contains(",gall,"))
                                        {
                                            std::thread::sleep(Duration::from_millis(80));
                                            suppress_hyprbars_for_gall();
                                            refresh_and_emit(&app, &paths, &shared, true);
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                            if received_any {
                                backoff_ms = 250;
                            }
                            // Sleep before reconnecting so immediate-EOF sockets cannot spin CPU.
                            std::thread::sleep(Duration::from_millis(backoff_ms));
                            if !received_any {
                                backoff_ms = (backoff_ms * 2).min(5_000);
                            }
                        }
                        Err(_) => {
                            std::thread::sleep(Duration::from_millis(backoff_ms));
                            backoff_ms = (backoff_ms * 2).min(5_000);
                        }
                    }
                }
            });
    }

    pub(super) fn compute_state(paths: &OmarchyPaths, query_hypr: bool) -> OmarchyState {
        let tiling_wm = is_tiling_wm();
        let active = is_omarchy_active_env(paths);
        if !active {
            return OmarchyState {
                tiling_wm,
                ..OmarchyState::default()
            };
        }

        let (theme_mode, palette_tokens) = match std::fs::read_to_string(&paths.colors_toml) {
            Ok(raw) => {
                let light_marker = paths.light_mode_marker.is_file();
                parse_colors_toml(&raw, light_marker).unwrap_or_else(|err| {
                    log::warn!(
                        "omarchy: failed to parse {}: {err}",
                        paths.colors_toml.display()
                    );
                    ("dark".to_string(), BTreeMap::new())
                })
            }
            Err(_) => ("dark".to_string(), BTreeMap::new()),
        };

        let custom_css = read_gall_css(&paths.gall_css);
        let glassy = read_glassy(&paths.glassy_lua);
        let monospace_font = read_monospace_font(&paths.fonts_conf);
        let language = Some(super::read_language(
            &paths.language_file,
            &super::omarchy_path().join("default/i18n/locales.json"),
        ));
        let rounding = if query_hypr && has_hyprland_env() {
            query_hyprctl_rounding().or(Some(0))
        } else if has_hyprland_env() {
            Some(0)
        } else {
            None
        };

        let floating_workspace = if query_hypr && has_hyprland_env() {
            query_hyprctl_clients_for_pid(std::process::id())
                .iter()
                .any(|c| c.on_floating_workspace)
        } else {
            false
        };

        OmarchyState {
            active: true,
            theme_mode,
            palette_tokens,
            custom_css,
            rounding,
            glassy,
            monospace_font,
            tiling_wm,
            floating_workspace,
            language,
        }
    }

    pub(super) fn read_gall_css(path: &Path) -> Option<String> {
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() || meta.len() > MAX_CUSTOM_CSS_BYTES {
            return None;
        }
        std::fs::read_to_string(path).ok()
    }

    pub(super) fn read_glassy(path: &Path) -> bool {
        path.is_file()
    }

    pub(super) fn read_monospace_font(fonts_conf: &Path) -> Option<String> {
        if let Ok(xml) = std::fs::read_to_string(fonts_conf) {
            if let Some(font) = parse_fonts_conf(&xml) {
                return Some(font);
            }
        }
        query_fc_match_monospace()
    }

    pub(super) fn run_command_timeout(mut cmd: Command, timeout: Duration) -> Option<String> {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().ok()?;
        let t0 = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        return None;
                    }
                    let out = child.wait_with_output().ok()?;
                    return String::from_utf8(out.stdout).ok();
                }
                Ok(None) if t0.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(15));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
    }

    pub(super) fn query_hyprctl_rounding() -> Option<u32> {
        let mut cmd = Command::new("hyprctl");
        cmd.args(["-j", "getoption", "decoration:rounding"]);
        let out = run_command_timeout(cmd, HYPRCTL_TIMEOUT)?;
        parse_hyprctl_rounding(&out)
    }

    fn query_fc_match_monospace() -> Option<String> {
        let mut cmd = Command::new("fc-match");
        cmd.args(["-f", "%{family[0]}", "monospace"]);
        let out = run_command_timeout(cmd, HYPRCTL_TIMEOUT)?;
        let font = out.trim();
        (!font.is_empty()).then(|| font.to_string())
    }

    pub(super) fn parse_hyprctl_rounding(json: &str) -> Option<u32> {
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        let n = v.get("int")?.as_i64()?;
        (n >= 0).then_some(n as u32)
    }

    pub(super) const FLOATING_WORKSPACE_TAG: &str = "omarchy-floating-workspace";

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct HyprClient {
        pub address: String,
        pub title: String,
        pub fullscreen: i64,
        pub on_floating_workspace: bool,
    }

    /// Lua rule that asks Omarchy's `hyprbars` plugin not to draw a second server-side title
    /// bar above Gall's own custom title bar (safe no-op when `hyprbars` is not loaded).
    pub(super) fn hyprbars_suppression_lua() -> &'static str {
        r#"if hl.plugin and hl.plugin.hyprbars then hl.window_rule({name="gall-no-hyprbars", enabled=true, match={class="^gall$"}, ["hyprbars:no_bar"]=true}) end"#
    }

    pub(super) fn suppress_hyprbars_for_gall() {
        if !has_hyprland_env() {
            return;
        }
        let mut cmd = Command::new("hyprctl");
        cmd.args(["eval", hyprbars_suppression_lua()]);
        let _ = run_command_timeout(cmd, Duration::from_millis(250));
    }

    pub(super) fn parse_hyprctl_clients_for_pid(json: &str, pid: u32) -> Vec<HyprClient> {
        let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(json)
        else {
            return Vec::new();
        };
        items
            .into_iter()
            .filter_map(|item| {
                let item_pid = u32::try_from(item.get("pid")?.as_u64()?).ok()?;
                if item_pid != pid {
                    return None;
                }
                let address = item.get("address")?.as_str()?.to_string();
                let title = item
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let fullscreen = item.get("fullscreen").and_then(|v| v.as_i64()).unwrap_or(0);
                let on_floating_workspace = item
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .is_some_and(|tags| {
                        tags.iter()
                            .filter_map(|t| t.as_str())
                            .any(|t| t == FLOATING_WORKSPACE_TAG)
                    });
                Some(HyprClient {
                    address,
                    title,
                    fullscreen,
                    on_floating_workspace,
                })
            })
            .collect()
    }

    pub(super) fn select_hypr_client<'a>(
        clients: &'a [HyprClient],
        title: Option<&str>,
    ) -> Option<&'a HyprClient> {
        if let Some(t) = title.filter(|s| !s.is_empty()) {
            if let Some(found) = clients.iter().find(|c| c.title == t) {
                return Some(found);
            }
        }
        clients
            .iter()
            .find(|c| c.on_floating_workspace)
            .or_else(|| clients.first())
    }

    fn query_hyprctl_clients_for_pid(pid: u32) -> Vec<HyprClient> {
        let mut cmd = Command::new("hyprctl");
        cmd.args(["clients", "-j"]);
        run_command_timeout(cmd, Duration::from_millis(250))
            .map(|out| parse_hyprctl_clients_for_pid(&out, pid))
            .unwrap_or_default()
    }

    pub(super) fn minimize_on_floating_workspace(title: Option<&str>) -> bool {
        if !has_hyprland_env() {
            return false;
        }
        let clients = query_hyprctl_clients_for_pid(std::process::id());
        let Some(client) = select_hypr_client(&clients, title) else {
            return false;
        };
        if !client.on_floating_workspace {
            return false;
        }
        let mut cmd = Command::new("omarchy-hyprland-window-minimize");
        cmd.arg(&client.address);
        run_command_timeout(cmd, Duration::from_millis(500)).is_some()
    }

    pub(super) fn is_maximized_on_floating_workspace(title: Option<&str>) -> Option<bool> {
        if !has_hyprland_env() {
            return None;
        }
        let clients = query_hyprctl_clients_for_pid(std::process::id());
        let client = select_hypr_client(&clients, title)?;
        client
            .on_floating_workspace
            .then_some(client.fullscreen == 1)
    }

    pub(super) fn toggle_maximize_on_floating_workspace(title: Option<&str>) -> Option<bool> {
        if !has_hyprland_env() {
            return None;
        }
        let clients = query_hyprctl_clients_for_pid(std::process::id());
        let client = select_hypr_client(&clients, title)?;
        if !client.on_floating_workspace {
            return None;
        }
        let lua = format!(
            r#"hl.dsp.window.fullscreen({{ mode = "maximized", action = "toggle", window = "address:{}" }})"#,
            client.address
        );
        let mut cmd = Command::new("hyprctl");
        cmd.args(["dispatch", &lua]);
        run_command_timeout(cmd, Duration::from_millis(250))?;
        Some(client.fullscreen != 1)
    }

    pub(super) fn parse_fonts_conf(xml: &str) -> Option<String> {
        let doc = roxmltree::Document::parse_with_options(
            xml,
            roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        )
        .ok()?;

        // 1. `omarchy-font-set` `<match><test name="family"><string>monospace</string></test><edit name="family"><string>NAME</string></edit></match>`
        for node in doc.descendants().filter(|n| n.has_tag_name("match")) {
            let matches_monospace = node
                .children()
                .filter(|c| c.has_tag_name("test") && c.attribute("name") == Some("family"))
                .any(|test| {
                    test.descendants()
                        .filter(|d| d.has_tag_name("string"))
                        .any(|s| {
                            s.text()
                                .is_some_and(|t| t.trim().eq_ignore_ascii_case("monospace"))
                        })
                });
            if !matches_monospace {
                continue;
            }
            for edit in node
                .children()
                .filter(|c| c.has_tag_name("edit") && c.attribute("name") == Some("family"))
            {
                if let Some(font) = edit
                    .descendants()
                    .find(|d| d.has_tag_name("string"))
                    .and_then(|s| s.text())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    return Some(font.to_string());
                }
            }
        }

        // 2. Standard fontconfig `<alias><family>monospace</family><prefer><family>NAME</family></prefer></alias>`
        for alias in doc.descendants().filter(|n| n.has_tag_name("alias")) {
            let is_monospace = alias
                .children()
                .filter(|c| c.has_tag_name("family"))
                .any(|f| {
                    f.text()
                        .is_some_and(|t| t.trim().eq_ignore_ascii_case("monospace"))
                });
            if !is_monospace {
                continue;
            }
            for prefer in alias.children().filter(|c| c.has_tag_name("prefer")) {
                if let Some(font) = prefer
                    .children()
                    .find(|c| c.has_tag_name("family"))
                    .and_then(|f| f.text())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    return Some(font.to_string());
                }
            }
        }

        None
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Rgb {
        r: u8,
        g: u8,
        b: u8,
    }

    impl Rgb {
        fn parse(raw: &str) -> Option<Self> {
            let s = raw.trim();
            let hex = s
                .strip_prefix('#')
                .or_else(|| s.strip_prefix("0x"))
                .or_else(|| s.strip_prefix("0X"))?;
            if hex.len() == 3 && hex.is_ascii() {
                let b = hex.as_bytes();
                let r =
                    u8::from_str_radix(&format!("{}{}", b[0] as char, b[0] as char), 16).ok()?;
                let g =
                    u8::from_str_radix(&format!("{}{}", b[1] as char, b[1] as char), 16).ok()?;
                let bl =
                    u8::from_str_radix(&format!("{}{}", b[2] as char, b[2] as char), 16).ok()?;
                return Some(Self { r, g, b: bl });
            }
            if (hex.len() == 6 || hex.len() == 8) && hex.is_ascii() {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                return Some(Self { r, g, b });
            }
            None
        }

        fn hex(self) -> String {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        }

        fn rgba(self, alpha: f32) -> String {
            format!("rgba({}, {}, {}, {:.2})", self.r, self.g, self.b, alpha)
        }

        fn mix(self, other: Self, weight_other: f32) -> Self {
            let t = weight_other.clamp(0.0, 1.0);
            let inv = 1.0 - t;
            Self {
                r: (f32::from(self.r) * inv + f32::from(other.r) * t).round() as u8,
                g: (f32::from(self.g) * inv + f32::from(other.g) * t).round() as u8,
                b: (f32::from(self.b) * inv + f32::from(other.b) * t).round() as u8,
            }
        }

        fn sum(self) -> u16 {
            u16::from(self.r) + u16::from(self.g) + u16::from(self.b)
        }

        fn luminance(self) -> f64 {
            let chan = |v: u8| {
                let c = f64::from(v) / 255.0;
                if c <= 0.03928 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * chan(self.r) + 0.7152 * chan(self.g) + 0.0722 * chan(self.b)
        }

        fn contrast_ratio(self, other: Self) -> f64 {
            let l1 = self.luminance();
            let l2 = other.luminance();
            let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
            (hi + 0.05) / (lo + 0.05)
        }
    }

    const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };
    const WHITE: Rgb = Rgb {
        r: 255,
        g: 255,
        b: 255,
    };

    fn insert_if_absent(map: &mut HashMap<String, String>, key: &str, val: &str) {
        if !map.contains_key(key) {
            map.insert(key.to_string(), val.to_string());
        }
    }

    fn extract_flat_strings(raw_toml: &str) -> Result<HashMap<String, String>, String> {
        let table: toml::Table = raw_toml.parse().map_err(|e| format!("{e}"))?;
        let mut out = HashMap::new();

        for (k, v) in &table {
            if let Some(s) = v.as_str() {
                out.insert(k.to_ascii_lowercase(), s.trim().to_string());
            }
        }

        // Legacy [colors.*] tables (Alacritty-style).
        if let Some(colors) = table.get("colors").and_then(|v| v.as_table()) {
            if let Some(primary) = colors.get("primary").and_then(|v| v.as_table()) {
                if let Some(s) = primary.get("background").and_then(|v| v.as_str()) {
                    insert_if_absent(&mut out, "background", s.trim());
                }
                if let Some(s) = primary.get("foreground").and_then(|v| v.as_str()) {
                    insert_if_absent(&mut out, "foreground", s.trim());
                }
            }
            if let Some(sel) = colors.get("selection").and_then(|v| v.as_table()) {
                if let Some(s) = sel.get("background").and_then(|v| v.as_str()) {
                    insert_if_absent(&mut out, "selection_background", s.trim());
                }
                if let Some(s) = sel.get("foreground").and_then(|v| v.as_str()) {
                    insert_if_absent(&mut out, "selection_foreground", s.trim());
                }
            }
            let ansi_names = [
                ("black", 0),
                ("red", 1),
                ("green", 2),
                ("yellow", 3),
                ("blue", 4),
                ("magenta", 5),
                ("cyan", 6),
                ("white", 7),
            ];
            if let Some(normal) = colors.get("normal").and_then(|v| v.as_table()) {
                for (name, idx) in ansi_names {
                    if let Some(s) = normal.get(name).and_then(|v| v.as_str()) {
                        insert_if_absent(&mut out, &format!("color{idx}"), s.trim());
                        if name != "black" && name != "white" {
                            insert_if_absent(&mut out, name, s.trim());
                        }
                    }
                }
            }
            if let Some(bright) = colors.get("bright").and_then(|v| v.as_table()) {
                for (name, idx) in ansi_names {
                    if let Some(s) = bright.get(name).and_then(|v| v.as_str()) {
                        insert_if_absent(&mut out, &format!("color{}", idx + 8), s.trim());
                        if name != "black" && name != "white" {
                            insert_if_absent(&mut out, &format!("bright_{name}"), s.trim());
                        }
                    }
                }
            }
        }

        // Legacy short aliases.
        for (alias, canon) in [
            ("bg", "background"),
            ("dark_bg", "dark_background"),
            ("darker_bg", "darker_background"),
            ("lighter_bg", "lighter_background"),
            ("fg", "foreground"),
            ("dark_fg", "dark_foreground"),
            ("light_fg", "light_foreground"),
            ("bright_fg", "bright_foreground"),
        ] {
            if let Some(val) = out.get(alias).cloned() {
                insert_if_absent(&mut out, canon, &val);
            }
        }

        Ok(out)
    }

    fn resolve_color(raw: &HashMap<String, String>, keys: &[&str]) -> Option<Rgb> {
        for key in keys {
            if let Some(val) = raw.get(*key) {
                if let Some(rgb) = Rgb::parse(val) {
                    return Some(rgb);
                }
            }
        }
        None
    }

    pub(super) fn parse_colors_toml(
        raw_toml: &str,
        light_marker: bool,
    ) -> Result<(String, BTreeMap<String, String>), String> {
        let raw = extract_flat_strings(raw_toml)?;

        let bg = resolve_color(&raw, &["background", "color0"]).unwrap_or(Rgb {
            r: 30,
            g: 30,
            b: 46,
        });
        let fg = resolve_color(&raw, &["foreground", "color7"]).unwrap_or(Rgb {
            r: 205,
            g: 214,
            b: 244,
        });

        let red = resolve_color(&raw, &["red", "color1"]).unwrap_or(Rgb {
            r: 224,
            g: 82,
            b: 82,
        });
        let green = resolve_color(&raw, &["green", "color2"]).unwrap_or(Rgb {
            r: 79,
            g: 179,
            b: 114,
        });
        let yellow = resolve_color(&raw, &["yellow", "color3"]).unwrap_or(Rgb {
            r: 217,
            g: 155,
            b: 38,
        });
        let blue = resolve_color(&raw, &["blue", "color4"]).unwrap_or(Rgb {
            r: 61,
            g: 139,
            b: 253,
        });
        let magenta = resolve_color(&raw, &["magenta", "color5", "purple"])
            .unwrap_or_else(|| blue.mix(red, 0.45));
        let cyan =
            resolve_color(&raw, &["cyan", "color6"]).unwrap_or_else(|| blue.mix(green, 0.45));

        let bright_yellow = resolve_color(&raw, &["bright_yellow", "color11"])
            .unwrap_or_else(|| yellow.mix(WHITE, 0.20));
        let bright_blue = resolve_color(&raw, &["bright_blue", "color12"])
            .unwrap_or_else(|| blue.mix(WHITE, 0.20));
        let accent = resolve_color(&raw, &["accent", "color4", "blue"]).unwrap_or(blue);

        let light_fg = resolve_color(&raw, &["light_foreground", "color7"]).unwrap_or(fg);
        let lighter_bg = resolve_color(&raw, &["lighter_background", "color0"]).unwrap_or(bg);
        let dark_fg = resolve_color(&raw, &["dark_foreground", "color8"]).unwrap_or(fg);
        let muted = resolve_color(&raw, &["muted", "color8"]).unwrap_or(dark_fg);
        let selection = resolve_color(
            &raw,
            &["selection", "selection_background", "color8", "color0"],
        )
        .unwrap_or(bg);
        let orange = resolve_color(&raw, &["orange"]).unwrap_or(yellow);
        let dark_bg =
            resolve_color(&raw, &["dark_background"]).unwrap_or_else(|| bg.mix(BLACK, 0.25));

        // Mode precedence: `mode` > `theme_type` > `light.mode` file > RGB sum > 382 > "dark".
        let mode_str = |v: &str| -> Option<&'static str> {
            match v.trim().to_ascii_lowercase().as_str() {
                "light" => Some("light"),
                "dark" => Some("dark"),
                _ => None,
            }
        };
        let theme_mode = raw
            .get("mode")
            .and_then(|v| mode_str(v))
            .or_else(|| raw.get("theme_type").and_then(|v| mode_str(v)))
            .or_else(|| light_marker.then_some("light"))
            .unwrap_or_else(|| if bg.sum() > 382 { "light" } else { "dark" })
            .to_string();
        let is_light = theme_mode == "light";

        // Contrast-safe surface and text tokens.
        let canvas_bg = if is_light {
            if dark_bg != bg && dark_bg.contrast_ratio(bg) <= 1.25 {
                dark_bg
            } else {
                bg.mix(fg, 0.04)
            }
        } else if dark_bg != bg {
            dark_bg
        } else {
            bg.mix(BLACK, 0.20)
        };

        let surface_raised = if is_light {
            if lighter_bg != bg && lighter_bg.contrast_ratio(fg) >= 5.5 {
                lighter_bg
            } else {
                bg.mix(fg, 0.08)
            }
        } else if lighter_bg != bg && lighter_bg.contrast_ratio(fg) >= 4.5 {
            lighter_bg
        } else {
            bg.mix(fg, 0.08)
        };

        let bg_hover = if selection != bg && selection.contrast_ratio(fg) >= 4.5 {
            selection
        } else {
            bg.mix(fg, 0.11)
        };

        let fg_dim = if light_fg != fg && light_fg.contrast_ratio(bg) >= 4.5 {
            light_fg
        } else {
            fg.mix(bg, 0.28)
        };

        let fg_faint = if muted != bg && muted != fg && muted.contrast_ratio(bg) >= 3.0 {
            muted
        } else {
            fg.mix(bg, 0.45)
        };

        let border = if muted != bg && (1.25..=5.5).contains(&muted.contrast_ratio(bg)) {
            muted
        } else {
            fg.mix(bg, 0.18)
        };

        let safe_accent = if accent.contrast_ratio(bg) >= 2.2 {
            accent
        } else {
            accent.mix(fg, 0.45)
        };
        let safe_brand_b = if magenta.contrast_ratio(bg) >= 2.0 {
            magenta
        } else {
            bright_blue
        };
        let safe_accent_2 = if cyan.contrast_ratio(bg) >= 2.0 {
            cyan
        } else {
            green
        };

        let mark_fg = if bg.contrast_ratio(yellow) >= fg.contrast_ratio(yellow) {
            bg
        } else {
            fg
        };

        let mut tokens = BTreeMap::new();
        tokens.insert("--bg".into(), canvas_bg.hex());
        tokens.insert("--bg-2".into(), bg.hex());
        tokens.insert("--bg-3".into(), surface_raised.hex());
        tokens.insert("--bg-hover".into(), bg_hover.hex());
        tokens.insert("--fg".into(), fg.hex());
        tokens.insert("--fg-dim".into(), fg_dim.hex());
        tokens.insert("--fg-faint".into(), fg_faint.hex());
        tokens.insert("--border".into(), border.hex());
        tokens.insert(
            "--border-soft".into(),
            fg.rgba(if is_light { 0.09 } else { 0.07 }),
        );
        tokens.insert("--glass".into(), bg.rgba(0.84));
        tokens.insert("--hover".into(), fg.rgba(0.06));
        tokens.insert("--accent".into(), safe_accent.hex());
        tokens.insert("--brand-b".into(), safe_brand_b.hex());
        tokens.insert("--accent-2".into(), safe_accent_2.hex());
        tokens.insert("--danger".into(), red.hex());
        tokens.insert("--ok".into(), green.hex());
        tokens.insert("--warn".into(), yellow.hex());
        tokens.insert("--folder".into(), blue.hex());
        tokens.insert("--mark".into(), yellow.hex());
        tokens.insert("--mark-fg".into(), mark_fg.hex());

        tokens.insert("--sm-cpu".into(), blue.hex());
        tokens.insert("--sm-gpu".into(), orange.hex());
        tokens.insert("--sm-mem".into(), green.hex());

        tokens.insert("--sx-comment".into(), fg_faint.hex());
        tokens.insert("--sx-string".into(), green.hex());
        tokens.insert("--sx-number".into(), orange.hex());
        tokens.insert("--sx-keyword".into(), magenta.hex());
        tokens.insert("--sx-operator".into(), cyan.hex());
        tokens.insert("--sx-function".into(), blue.hex());
        tokens.insert("--sx-type".into(), yellow.hex());
        tokens.insert("--sx-variable".into(), red.hex());
        tokens.insert("--sx-attr".into(), bright_yellow.hex());

        Ok((theme_mode, tokens))
    }
}

/// Whether the window starts maximized: the setting, except under a tiling window manager
/// (which sizes windows itself and treats a forced maximize as noise).
pub fn start_maximized(setting: bool, tiling_wm: bool) -> bool {
    setting && !tiling_wm
}

/// Minimizes the calling Gall window to Omarchy's floating-workspace dock when running on an
/// Omarchy floating workspace under Hyprland (which ignores `xdg_toplevel.set_minimized`).
/// Returns `true` when Omarchy handled the minimize, `false` to fall back to native `minimize()`.
#[cfg(target_os = "linux")]
pub fn minimize_window_on_omarchy_floating(title: Option<&str>) -> bool {
    linux::minimize_on_floating_workspace(title)
}

/// Returns Hyprland's real maximized state (`fullscreen == 1`) when running on an Omarchy
/// floating workspace, because GTK3's `WindowState::MAXIMIZED` can stay stale after a window
/// transitions from tiled to floating.
#[cfg(target_os = "linux")]
pub fn is_maximized_on_omarchy_floating(title: Option<&str>) -> Option<bool> {
    linux::is_maximized_on_floating_workspace(title)
}

/// Toggles Hyprland's maximized state (`mode = "maximized"`) on an Omarchy floating workspace,
/// matching Omarchy's `hyprbars` maximize button and double-click behaviour.
#[cfg(target_os = "linux")]
pub fn toggle_maximize_on_omarchy_floating(title: Option<&str>) -> Option<bool> {
    linux::toggle_maximize_on_floating_workspace(title)
}

/// Installs an in-memory rule that matches only Gall's transient startup title.
#[cfg(gall_wayland_presentation)]
pub fn prepare_window_reveal() -> bool {
    if !crate::window_presentation::available() {
        return false;
    }
    let mut cmd = std::process::Command::new("hyprctl");
    cmd.args([
        "eval",
        r#"hl.window_rule({name="gall-first-frame", enabled=true, match={class="^gall$", title="^Gall [(]preparing window[)]$"}, opacity="0.01 override 0.01 override 0.01 override", no_blur=true, no_anim=true, no_shadow=true, render_unfocused=true})"#,
    ]);
    linux::run_command_timeout(cmd, std::time::Duration::from_millis(250))
        .is_some_and(|output| output.trim() == "ok")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_maximized_is_suppressed_by_tiling_wms() {
        assert!(start_maximized(true, false));
        assert!(!start_maximized(true, true));
        assert!(!start_maximized(false, false));
        assert!(!start_maximized(false, true));
    }

    #[test]
    fn is_tiling_wm_with_checks_env_matrix() {
        let none = |_: &str| None;
        assert!(!is_tiling_wm_with(none));

        assert!(is_tiling_wm_with(
            |k| (k == "HYPRLAND_INSTANCE_SIGNATURE").then(|| "sig123".to_string())
        ));
        assert!(is_tiling_wm_with(
            |k| (k == "SWAYSOCK").then(|| "/run/user/1000/sway.sock".to_string())
        ));
        assert!(is_tiling_wm_with(
            |k| (k == "NIRI_SOCKET").then(|| "/run/user/1000/niri.sock".to_string())
        ));
        assert!(is_tiling_wm_with(
            |k| (k == "I3SOCK").then(|| "/run/user/1000/i3.sock".to_string())
        ));
        assert!(is_tiling_wm_with(
            |k| (k == "XDG_CURRENT_DESKTOP").then(|| "Hyprland:uwsm".to_string())
        ));
        assert!(is_tiling_wm_with(
            |k| (k == "XDG_SESSION_DESKTOP").then(|| "sway".to_string())
        ));
        assert!(!is_tiling_wm_with(
            |k| (k == "XDG_CURRENT_DESKTOP").then(|| "KDE".to_string())
        ));
        assert!(!is_tiling_wm_with(
            |k| (k == "HYPRLAND_INSTANCE_SIGNATURE").then(|| "   ".to_string())
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_colors_toml_canonical_and_legacy_cascade() {
        let canonical = r##"
            accent = "#89b4fa"
            background = "#1e1e2e"
            foreground = "#cdd6f4"
            color0 = "#45475a"
            color1 = "#f38ba8"
            color2 = "#a6e3a1"
            color3 = "#f9e2af"
            color4 = "#89b4fa"
            color5 = "#f5c2e7"
            color6 = "#94e2d5"
            color7 = "#bac2de"
            color8 = "#585b70"
        "##;
        let (mode, tokens) = linux::parse_colors_toml(canonical, false).unwrap();
        assert_eq!(mode, "dark");
        assert_eq!(tokens.get("--bg-2").map(String::as_str), Some("#1e1e2e"));
        assert_eq!(tokens.get("--fg").map(String::as_str), Some("#cdd6f4"));
        assert_eq!(tokens.get("--accent").map(String::as_str), Some("#89b4fa"));
        assert_eq!(
            tokens.get("--sx-string").map(String::as_str),
            Some("#a6e3a1")
        );

        // Legacy bg/fg + [colors.normal] + accent <- color4 fallback.
        let legacy = r##"
            bg = "#101218"
            fg = "#e0e4ec"
            [colors.normal]
            red = "#cc4444"
            green = "#44cc77"
            yellow = "#ccaa33"
            blue = "#3388ee"
            magenta = "#aa55cc"
            cyan = "#44bbcc"
        "##;
        let (mode2, tokens2) = linux::parse_colors_toml(legacy, false).unwrap();
        assert_eq!(mode2, "dark");
        assert_eq!(tokens2.get("--bg-2").map(String::as_str), Some("#101218"));
        assert_eq!(tokens2.get("--fg").map(String::as_str), Some("#e0e4ec"));
        assert_eq!(tokens2.get("--accent").map(String::as_str), Some("#3388ee"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_colors_toml_mode_precedence_and_white_theme_contrast() {
        // Explicit `mode` overrides `theme_type`, `light.mode`, and RGB sum.
        let (m1, _) = linux::parse_colors_toml(
            r##"
            mode = "dark"
            theme_type = "light"
            background = "#ffffff"
            foreground = "#000000"
        "##,
            true,
        )
        .unwrap();
        assert_eq!(m1, "dark");

        // `theme_type` overrides `light.mode` and RGB sum.
        let (m2, _) = linux::parse_colors_toml(
            r##"
            theme_type = "light"
            background = "#111111"
            foreground = "#eeeeee"
        "##,
            false,
        )
        .unwrap();
        assert_eq!(m2, "light");

        // `light.mode` overrides dark RGB sum.
        let (m3, _) = linux::parse_colors_toml(
            r##"
            background = "#111111"
            foreground = "#eeeeee"
        "##,
            true,
        )
        .unwrap();
        assert_eq!(m3, "light");

        // RGB sum > 382 -> light; <= 382 -> dark.
        let (m4, _) = linux::parse_colors_toml(
            r##"
            background = "#808081"
            foreground = "#111111"
        "##,
            false,
        )
        .unwrap();
        assert_eq!(m4, "light");

        let (m5, _) = linux::parse_colors_toml(
            r##"
            background = "#7f7f80"
            foreground = "#eeeeee"
        "##,
            false,
        )
        .unwrap();
        assert_eq!(m5, "dark");

        // Contrast-safe white theme (`#ffffff` bg / `#000000` fg, `#c0c0c0` dark_foreground).
        let (m_white, white_tokens) = linux::parse_colors_toml(
            r##"
            background = "#ffffff"
            foreground = "#000000"
            dark_foreground = "#c0c0c0"
            lighter_background = "#c0c0c0"
            accent = "#ffffff"
        "##,
            false,
        )
        .unwrap();
        assert_eq!(m_white, "light");
        assert_ne!(white_tokens.get("--fg-dim").unwrap(), "#c0c0c0");
        assert_ne!(white_tokens.get("--fg-faint").unwrap(), "#c0c0c0");
        assert_ne!(white_tokens.get("--accent").unwrap(), "#ffffff");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_hyprctl_fonts_conf_and_glassy() {
        assert_eq!(
            linux::parse_hyprctl_rounding(r#"{"option":"decoration:rounding","int":8,"set":true}"#),
            Some(8)
        );
        assert_eq!(
            linux::parse_hyprctl_rounding(r#"{"option":"decoration:rounding","int":0}"#),
            Some(0)
        );
        assert_eq!(
            linux::parse_hyprctl_rounding(r#"{"option":"decoration:rounding","int":-1}"#),
            None
        );
        assert_eq!(linux::parse_hyprctl_rounding("not json"), None);

        // `omarchy-font-set` XML with `<!DOCTYPE fontconfig SYSTEM "fonts.dtd">`.
        let omarchy_xml = r#"<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <match target="pattern">
    <test name="family" qual="any">
      <string>monospace</string>
    </test>
    <edit name="family" mode="prepend_first" binding="strong">
      <string>JetBrainsMono Nerd Font</string>
    </edit>
  </match>
</fontconfig>"#;
        assert_eq!(
            linux::parse_fonts_conf(omarchy_xml).as_deref(),
            Some("JetBrainsMono Nerd Font")
        );

        let alias_xml = r#"<?xml version="1.0"?>
<fontconfig>
  <alias>
    <family>monospace</family>
    <prefer><family>CaskaydiaMono Nerd Font</family></prefer>
  </alias>
</fontconfig>"#;
        assert_eq!(
            linux::parse_fonts_conf(alias_xml).as_deref(),
            Some("CaskaydiaMono Nerd Font")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_watcher_survives_atomic_theme_directory_swap() {
        use notify_debouncer_full::notify::{Config as NotifyConfig, RecommendedWatcher};
        use notify_debouncer_full::{new_debouncer_opt, DebounceEventResult, NoCache};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;

        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "gall-omarchy-watch-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let state_base = root.join("state");
        let config_base = root.join("config");
        let paths = linux::OmarchyPaths::from_bases(&state_base, &config_base);

        std::fs::create_dir_all(&paths.theme_dir).unwrap();
        std::fs::create_dir_all(&paths.fonts_dir).unwrap();
        std::fs::create_dir_all(&paths.glassy_dir).unwrap();
        std::fs::write(
            &paths.colors_toml,
            "background = \"#111111\"\nforeground = \"#eeeeee\"\n",
        )
        .unwrap();

        let (tx, rx) = std::sync::mpsc::channel::<DebounceEventResult>();
        let mut debouncer = new_debouncer_opt::<_, RecommendedWatcher, NoCache>(
            Duration::from_millis(40),
            None,
            tx,
            NoCache::new(),
            NotifyConfig::default(),
        )
        .unwrap();
        let mut watched = Vec::new();
        // Arm initial watches using the same logic as the bridge.
        for p in [
            &paths.current_dir,
            &paths.theme_dir,
            &paths.colors_toml,
            &paths.glassy_dir,
        ] {
            debouncer
                .watch(
                    p,
                    notify_debouncer_full::notify::RecursiveMode::NonRecursive,
                )
                .unwrap();
            watched.push(p.clone());
        }

        let perform_swap = |colors: &str, suffix: &str| {
            let next = paths.current_dir.join(format!("next-theme-{suffix}"));
            std::fs::create_dir_all(&next).unwrap();
            std::fs::write(next.join("colors.toml"), colors).unwrap();
            std::fs::remove_dir_all(&paths.theme_dir).unwrap();
            std::fs::rename(&next, &paths.theme_dir).unwrap();
        };

        // Swap 1: dark -> light theme via atomic directory replacement.
        perform_swap("background = \"#fafafa\"\nforeground = \"#111111\"\n", "1");
        let ev1 = rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
        assert!(!ev1.is_empty());
        // Re-arm watches as the bridge does on debounced reload.
        for prev in watched.drain(..) {
            let _ = debouncer.unwatch(&prev);
        }
        for p in [&paths.current_dir, &paths.theme_dir, &paths.colors_toml] {
            debouncer
                .watch(
                    p,
                    notify_debouncer_full::notify::RecursiveMode::NonRecursive,
                )
                .unwrap();
            watched.push(p.clone());
        }
        while rx.try_recv().is_ok() {}
        let (mode1, _) =
            linux::parse_colors_toml(&std::fs::read_to_string(&paths.colors_toml).unwrap(), false)
                .unwrap();
        assert_eq!(mode1, "light");

        // Swap 2: light -> dark theme (verifies re-armed watches survived swap 1!).
        perform_swap("background = \"#121212\"\nforeground = \"#e0e0e0\"\n", "2");
        let ev2 = rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
        assert!(!ev2.is_empty());
        let (mode2, _) =
            linux::parse_colors_toml(&std::fs::read_to_string(&paths.colors_toml).unwrap(), false)
                .unwrap();
        assert_eq!(mode2, "dark");

        // `gall.css` & `window-glassy.lua` helpers.
        assert!(linux::read_gall_css(&paths.gall_css).is_none());
        std::fs::write(&paths.gall_css, "/* custom */").unwrap();
        assert_eq!(
            linux::read_gall_css(&paths.gall_css).as_deref(),
            Some("/* custom */")
        );
        assert!(!linux::read_glassy(&paths.glassy_lua));
        std::fs::write(&paths.glassy_lua, "-- glassy").unwrap();
        assert!(linux::read_glassy(&paths.glassy_lua));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn omarchy_state_serializes_with_expected_camel_case_keys() {
        let mut palette_tokens = BTreeMap::new();
        palette_tokens.insert("--bg".into(), "#ffffff".into());
        let state = OmarchyState {
            active: true,
            theme_mode: "light".into(),
            palette_tokens,
            custom_css: Some("body {}".into()),
            rounding: Some(8),
            glassy: true,
            monospace_font: Some("JetBrainsMono Nerd Font".into()),
            ..Default::default()
        };

        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["active"], true);
        assert_eq!(json["themeMode"], "light");
        assert_eq!(json["rounding"], 8);
        assert_eq!(json["glassy"], true);
        assert_eq!(json["monospaceFont"], "JetBrainsMono Nerd Font");
        assert_eq!(json["customCss"], "body {}");
        assert_eq!(json["paletteTokens"]["--bg"], "#ffffff");
        assert!(json.get("tilingWm").is_some());
        assert_eq!(json["floatingWorkspace"], false);
        assert!(json["language"].is_null());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_hyprctl_clients_extracts_pid_and_floating_workspace_tag() {
        let sample = r#"[
            {"address":"0x111","pid":999,"title":"Other","fullscreen":1,"tags":["omarchy-floating-workspace"]},
            {"address":"0x222","pid":4242,"title":"/home/u — Gall","fullscreen":0,"tags":[]},
            {"address":"0x333","pid":4242,"title":"/tmp — Gall","fullscreen":1,"tags":["omarchy-floating-workspace","omarchy-mode-floated"]}
        ]"#;
        let clients = linux::parse_hyprctl_clients_for_pid(sample, 4242);
        assert_eq!(clients.len(), 2);
        assert_eq!(clients[0].address, "0x222");
        assert_eq!(clients[0].fullscreen, 0);
        assert!(!clients[0].on_floating_workspace);
        assert_eq!(clients[1].address, "0x333");
        assert_eq!(clients[1].fullscreen, 1);
        assert!(clients[1].on_floating_workspace);

        assert_eq!(
            linux::select_hypr_client(&clients, Some("/home/u — Gall")).map(|c| c.address.as_str()),
            Some("0x222")
        );
        assert_eq!(
            linux::select_hypr_client(&clients, Some("/tmp — Gall")).map(|c| c.address.as_str()),
            Some("0x333")
        );
        assert_eq!(
            linux::select_hypr_client(&clients, None).map(|c| c.address.as_str()),
            Some("0x333")
        );
        assert!(linux::hyprbars_suppression_lua().contains("hyprbars:no_bar"));
    }

    #[test]
    fn language_comes_from_the_language_file_and_locales_json() {
        let dir = std::env::temp_dir().join(format!("gall-omarchy-lang-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let locales = dir.join("locales.json");
        std::fs::write(
            &locales,
            r#"{"en":{"formatLocale":"en-US"},"sl":{"formatLocale":"sl-SI"},"ar":{"formatLocale":"ar-AE","direction":"rtl"}}"#,
        )
        .unwrap();
        let file = dir.join("language");
        let lang = |text: &str| {
            std::fs::write(&file, text).unwrap();
            read_language(&file, &locales)
        };
        let sl = lang("sl\n");
        assert_eq!(
            (
                sl.code.as_str(),
                sl.format_locale.as_str(),
                sl.direction.as_str()
            ),
            ("sl", "sl-SI", "ltr")
        );
        assert_eq!(lang("  ar  \nignored").direction, "rtl");
        assert_eq!(lang("xx").code, "en", "unknown code means English");
        assert_eq!(lang("").code, "en");
        assert_eq!(read_language(&dir.join("missing"), &locales).code, "en");
        assert_eq!(read_language(&file, &dir.join("missing.json")).code, "en");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn watcher_ignores_read_only_access_events() {
        use notify_debouncer_full::notify::{
            event::{AccessKind, AccessMode, CreateKind, ModifyKind},
            Event, EventKind,
        };
        for access in [
            AccessKind::Any,
            AccessKind::Read,
            AccessKind::Open(AccessMode::Any),
            AccessKind::Open(AccessMode::Read),
            AccessKind::Close(AccessMode::Any),
            AccessKind::Close(AccessMode::Read),
        ] {
            assert!(
                !linux::is_mutation_event(&Event::new(EventKind::Access(access))),
                "read-only access {access:?} must not retrigger omarchy-watch"
            );
        }
        assert!(linux::is_mutation_event(&Event::new(EventKind::Access(
            AccessKind::Close(AccessMode::Write)
        ))));
        assert!(linux::is_mutation_event(&Event::new(EventKind::Create(
            CreateKind::File
        ))));
        assert!(linux::is_mutation_event(&Event::new(EventKind::Modify(
            ModifyKind::Any
        ))));
    }
}
