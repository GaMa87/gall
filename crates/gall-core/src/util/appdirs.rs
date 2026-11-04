//! Locations of configuration and data (C-02, C-03).
//!
//! Portable mode: when a `portable` marker file sits next to the executable,
//! everything is stored in `<exe dir>/UserData` instead of the user profile
//! (the app shell also puts its logs and the webview's data there).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppDirs {
    pub config: PathBuf,
    pub data: PathBuf,
    /// True when running from a folder with a `portable` marker.
    pub portable: bool,
}

impl AppDirs {
    pub fn resolve(exe_dir: Option<&Path>) -> Self {
        if let Some(d) = exe_dir {
            if d.join("portable").exists() {
                let mut dirs = Self::at(&d.join("UserData"));
                dirs.portable = true;
                return dirs;
            }
        }
        let config = dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("gall");
        let data = dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("gall");
        AppDirs {
            config,
            data,
            portable: false,
        }
    }

    pub fn at(root: &Path) -> Self {
        AppDirs {
            config: root.to_path_buf(),
            data: root.to_path_buf(),
            portable: false,
        }
    }

    /// The webview's own data (localStorage, cache).
    pub fn webview(&self) -> PathBuf {
        self.data.join("WebView")
    }

    pub fn settings(&self) -> PathBuf {
        self.config.join("settings.json")
    }
    pub fn favorites(&self) -> PathBuf {
        self.config.join("favorites.json")
    }
    pub fn session(&self) -> PathBuf {
        self.config.join("session.json")
    }
    pub fn templates(&self) -> PathBuf {
        self.config.join("templates")
    }
    pub fn themes(&self) -> PathBuf {
        self.config.join("themes")
    }
    pub fn database(&self) -> PathBuf {
        self.data.join("gall.db")
    }
    pub fn logs(&self) -> PathBuf {
        self.data.join("logs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_marker_keeps_everything_next_to_the_exe() {
        let t = tempfile::tempdir().unwrap();
        assert!(!AppDirs::resolve(Some(t.path())).portable);
        std::fs::write(t.path().join("portable"), "").unwrap();
        let d = AppDirs::resolve(Some(t.path()));
        let root = t.path().join("UserData");
        assert!(d.portable);
        assert_eq!(d.settings(), root.join("settings.json"));
        assert!(d.webview().starts_with(&root) && d.logs().starts_with(&root));
    }
}
