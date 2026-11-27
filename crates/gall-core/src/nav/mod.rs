//! Navigation model: per-tab history, tabs, browsers and the window session.

pub mod history;
pub mod path;
pub mod session;

pub use history::History;
pub use session::{
    Browser, FlowDirection, FlowOp, Layout, PairMode, Session, StoredSession, Tab, TabPair,
    Workspace, WorkspaceSnapshot, WorkspaceStore, WorkspaceSummary,
};
