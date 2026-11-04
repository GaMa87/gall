//! Filesystem engine: listing, drives, watching, search and operations.

pub mod archive;
pub mod archive_view;
pub mod cloud;
pub mod diff;
pub mod dircmp;
pub mod dirsize;
pub mod drives;
pub mod entry;
pub mod fuzzy;
pub mod gitstatus;
pub mod grep;
pub mod hexdiff;
pub mod index;
pub mod listing;
pub mod ops;
pub mod search;
pub mod syntax;
pub mod table;
pub mod trashbin;
pub mod volumes;
pub mod watcher;

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
