//! Storage engine: JSON documents (settings, favourites, marks, session) and SQLite
//! (per-folder preferences, history, tags, recent destinations).

pub mod compare_session;
pub mod db;
pub mod favorites;
pub mod favorites_import;
pub mod gravity;
pub mod json_store;
pub mod marks;
pub mod settings;

pub use db::Db;
pub use favorites::Favorites;
pub use gravity::{GravityReason, GravitySuggestion};
pub use json_store::JsonStore;
pub use marks::Marks;
pub use settings::{Settings, ThemeMode, WindowButtonsMode, WorkspaceBarMode};
