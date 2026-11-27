//! Filesystem engine: listing, drives, watching, search and operations.

pub mod archive;
pub mod archive_view;
pub mod cloud;
pub mod compare_pdf;
pub mod compare_report;
pub mod diff;
pub mod dircmp;
pub mod dirsize;
pub mod drives;
pub mod entry;
pub mod fuzzy;
pub mod gitabsorb;
pub mod gitadvanced;
pub mod gitblame;
pub mod gitcustom;
pub mod gitexec;
mod gitfacts;
pub mod gitforecast;
pub mod gitgraph;
pub mod gitimages;
pub mod gitinspect;
pub mod gitjournal;
pub mod gitlocation;
pub mod gitnetwork;
pub mod gitprogress;
pub mod gitreflog;
pub mod gitrepository;
pub mod gitrewrite;
pub mod gitsearch;
pub mod gitstash;
pub mod gitstatus;
pub mod gitwip;
mod gitworker;
pub mod gitworktree;
pub mod gitworktrees;
pub mod grep;
pub mod gvfs;
pub mod hexdiff;
pub mod ignorefile;
pub mod ignorerules;
pub mod index;
pub mod listing;
#[cfg(target_os = "macos")]
pub mod mactrash;
pub mod ops;
pub mod patch;
pub mod search;
pub mod search_filter;
pub mod structural;
pub mod syntax;
pub mod table;
pub mod trashbin;
pub mod volumes;
pub mod watcher;
pub mod wsl;

pub use entry::{EntryKind, FileEntry};

/// Opens `path` for reading only if it is a regular file (after symlinks). Opening a
/// named pipe waits until something writes to it, which can be forever; sockets and
/// devices are no files to preview, compare or search either.
pub fn open_regular(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file (a pipe, socket or device)",
        ));
    }
    std::fs::File::open(path)
}

/// [`open_regular`], read to the end.
pub fn read_regular(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut buf = Vec::new();
    open_regular(path)?.read_to_end(&mut buf)?;
    Ok(buf)
}
pub use listing::{list_dir, ListOptions};

/// Makes the Git of the unit tests ignore the configuration of the person who runs them (`pull.rebase`,
/// `diff.mnemonicPrefix`, aliases, hooks of a template, `init.defaultBranch`…): the global and system files are
/// replaced by an empty one for the whole test process (AGENTS.md asks for the same through the environment). Call it
/// first in a test that makes a repository; it works once and is cheap afterwards.
#[cfg(test)]
pub(crate) fn hermetic_git() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let empty =
            std::env::temp_dir().join(format!("gall-empty-gitconfig-{}", std::process::id()));
        let _ = std::fs::write(&empty, "");
        std::env::set_var("GIT_CONFIG_GLOBAL", &empty);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    });
}
