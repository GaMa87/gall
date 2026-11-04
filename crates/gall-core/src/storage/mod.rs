//! Storage engine: JSON documents (settings, favourites, session) and SQLite
//! (per-folder preferences, history, tags, recent destinations).

pub mod db;
pub mod favorites;
pub mod json_store;
pub mod settings;

pub use db::Db;
pub use favorites::Favorites;
pub use json_store::JsonStore;
pub use settings::Settings;
