//! Gall core: platform-independent file-manager logic.
//!
//! This crate deliberately has no UI or Tauri dependency so that every rule
//! (sorting, filtering, navigation history, settings safety, file operations)
//! can be unit-tested in isolation.

pub mod compare;
pub mod compare_ai;
pub mod diagnostics;
pub mod doc_compare;
pub mod error;
pub mod fs;
pub mod generated_compare;
pub mod handoff;
pub mod merge;
pub mod nav;
pub mod preview;
pub mod report_bundle;
pub mod semantic_compare;
pub mod storage;
pub mod sysmon;
pub mod updater;
pub mod util;
pub mod view;

pub use error::{CoreError, Result};
