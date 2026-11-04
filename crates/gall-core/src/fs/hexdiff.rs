//! Byte comparison of two files for the hex view (D-04): where they differ, as ranges of
//! offsets, found by streaming both files (any size; nothing is kept but the ranges).
//! Bytes are compared at the same offset, like `cmp` or a hex editor's compare: an
//! insertion shifts everything after it, which then differs.

use crate::error::{IoContext, Result};
use serde::Serialize;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Most ranges kept; past that only the count of differing bytes goes on.
pub const MAX_RANGES: usize = 100_000;
/// Most bytes one read returns (the UI asks for what it shows).
pub const MAX_READ: u64 = 256 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HexDiff {
    pub left_size: u64,
    pub right_size: u64,
    /// Differing byte ranges `[start, end)`, ascending; the tail of the longer file is one
    /// range. Bytes closer than [`MERGE_GAP`] apart are one range.
    pub ranges: Vec<(u64, u64)>,
    /// Bytes that differ, the longer file's tail included.
    pub differing: u64,
    /// More ranges than [`MAX_RANGES`]: the list stops, `differing` does not.
    pub truncated: bool,
}

/// Differences fewer than this many equal bytes apart count as one range (a changed
/// 32-bit value is one difference, not four).
pub const MERGE_GAP: u64 = 4;

/// Compares `left` and `right` byte by byte. None when cancelled.
pub fn compare(left: &Path, right: &Path, cancel: &AtomicBool) -> Result<Option<HexDiff>> {
    let mut a = super::open_regular(left).ctx("open", left)?;
    let mut b = super::open_regular(right).ctx("open", right)?;
    let mut out = HexDiff {
        left_size: a.metadata().ctx("stat", left)?.len(),
        right_size: b.metadata().ctx("stat", right)?.len(),
        ..Default::default()
    };
    const CHUNK: usize = 1 << 20;
    let (mut ba, mut bb) = (vec![0u8; CHUNK], vec![0u8; CHUNK]);
    let mut pos = 0u64;
    let mut open: Option<(u64, u64)> = None; // the range being built
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let na = read_full(&mut a, &mut ba).ctx("read", left)?;
        let nb = read_full(&mut b, &mut bb).ctx("read", right)?;
        let n = na.min(nb);
        for i in 0..n {
            if ba[i] != bb[i] {
                let at = pos + i as u64;
                out.differing += 1;
                open = match open {
                    Some((s, e)) if at < e + MERGE_GAP => Some((s, at + 1)),
                    Some(r) => {
                        push(&mut out, r);
                        Some((at, at + 1))
                    }
                    None => Some((at, at + 1)),
                };
            }
        }
        pos += n as u64;
        if na != nb || na < CHUNK {
            break; // one (or both) ended
        }
    }
    if let Some(r) = open {
        push(&mut out, r);
    }
    // The longer file's tail: everything there differs.
    let (short, long) = (
        out.left_size.min(out.right_size),
        out.left_size.max(out.right_size),
    );
    if long > short {
        out.differing += long - short;
        match out.ranges.last_mut() {
            Some(last) if last.1 + MERGE_GAP > short => last.1 = long,
            _ => push(&mut out, (short, long)),
        }
    }
    Ok(Some(out))
}

fn push(out: &mut HexDiff, r: (u64, u64)) {
    if out.ranges.len() < MAX_RANGES {
        out.ranges.push(r);
    } else {
        out.truncated = true;
    }
}

/// Fills `buf` as far as the file goes (a short read is not the end).
fn read_full(f: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

/// `len` bytes of `path` from `offset` (fewer at the end; at most [`MAX_READ`]).
pub fn read_at(path: &Path, offset: u64, len: u64) -> Result<Vec<u8>> {
    let mut f = super::open_regular(path).ctx("open", path)?;
    f.seek(SeekFrom::Start(offset)).ctx("read", path)?;
    let mut buf = vec![0u8; len.min(MAX_READ) as usize];
    let n = read_full(&mut f, &mut buf).ctx("read", path)?;
    buf.truncate(n);
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(a: &[u8], b: &[u8]) -> HexDiff {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a"), a).unwrap();
        std::fs::write(d.path().join("b"), b).unwrap();
        compare(
            &d.path().join("a"),
            &d.path().join("b"),
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap()
    }

    #[test]
    fn identical_files_have_no_ranges() {
        let r = diff(b"same bytes", b"same bytes");
        assert!(r.ranges.is_empty());
        assert_eq!(r.differing, 0);
    }

    #[test]
    fn nearby_bytes_merge_and_distant_ones_do_not() {
        //            0123456789012345678901
        let a = b"aaaaaaaaaaaaaaaaaaaaaa";
        let b = b"aaXaXaaaaaaaaaaaaaaYaa";
        let r = diff(a, b);
        assert_eq!(r.ranges, vec![(2, 5), (19, 20)]);
        assert_eq!(r.differing, 3);
    }

    #[test]
    fn the_longer_tail_differs() {
        let r = diff(b"abcdef", b"abcdefXYZ");
        assert_eq!(r.ranges, vec![(6, 9)]);
        assert_eq!(r.differing, 3);
        assert_eq!((r.left_size, r.right_size), (6, 9));
        // A difference right before the tail joins it.
        let r = diff(b"abcdeQ", b"abcdefXYZ");
        assert_eq!(r.ranges, vec![(5, 9)]);
        assert_eq!(r.differing, 4);
    }

    /// Differences across the 1 MiB chunk border are found and merged.
    #[test]
    fn differences_across_chunks() {
        let mut a = vec![0u8; (1 << 20) + 100];
        let mut b = a.clone();
        b[(1 << 20) - 1] = 1;
        b[1 << 20] = 1;
        a[(1 << 20) + 50] = 7;
        let r = diff(&a, &b);
        assert_eq!(
            r.ranges,
            vec![
                ((1 << 20) - 1, (1 << 20) + 1),
                ((1 << 20) + 50, (1 << 20) + 51)
            ]
        );
    }

    #[test]
    fn reads_a_window() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        std::fs::write(&p, b"0123456789").unwrap();
        assert_eq!(read_at(&p, 3, 4).unwrap(), b"3456");
        assert_eq!(read_at(&p, 8, 10).unwrap(), b"89");
        assert!(read_at(&p, 20, 4).unwrap().is_empty());
    }

    #[test]
    fn cancelled_returns_none() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a"), b"x").unwrap();
        let r = compare(
            &d.path().join("a"),
            &d.path().join("a"),
            &AtomicBool::new(true),
        )
        .unwrap();
        assert!(r.is_none());
    }
}
