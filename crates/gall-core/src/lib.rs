//! Gall core: platform-independent file-manager logic.
//!
//! This crate deliberately has no UI or Tauri dependency so that every rule
//! (sorting, filtering, navigation history, settings safety, file operations)
//! can be unit-tested in isolation.

pub mod error;
pub mod fs;
pub mod nav;
pub mod preview;
pub mod storage;
pub mod sysmon;
pub mod util;
pub mod view;

pub use error::{CoreError, Result};
