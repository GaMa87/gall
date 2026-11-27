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

/// On Linux, jump past a shared sparse hole in both files (`SEEK_DATA`) so multi-GB sparse
/// files compare in milliseconds without reading gigabytes of zero holes.
#[cfg(target_os = "linux")]
fn skip_shared_hole(a: &mut std::fs::File, b: &mut std::fs::File, pos: u64, short: u64) -> u64 {
    use std::os::fd::AsRawFd;
    const SEEK_DATA: libc::c_int = 3;
    let Ok(off) = libc::off_t::try_from(pos) else {
        return pos;
    };
    let next_data = |f: &std::fs::File| -> Option<u64> {
        let r = unsafe { libc::lseek(f.as_raw_fd(), off, SEEK_DATA) };
        if r >= 0 {
            Some((r as u64).min(short))
        } else if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENXIO) {
            Some(short)
        } else {
            None
        }
    };
    let (Some(da), Some(db)) = (next_data(a), next_data(b)) else {
        let _ = a.seek(SeekFrom::Start(pos));
        let _ = b.seek(SeekFrom::Start(pos));
        return pos;
    };
    let target = da.min(db).min(short);
    if target > pos
        && a.seek(SeekFrom::Start(target)).is_ok()
        && b.seek(SeekFrom::Start(target)).is_ok()
    {
        return target;
    }
    let _ = a.seek(SeekFrom::Start(pos));
    let _ = b.seek(SeekFrom::Start(pos));
    pos
}

#[cfg(not(target_os = "linux"))]
fn skip_shared_hole(_a: &mut std::fs::File, _b: &mut std::fs::File, pos: u64, _short: u64) -> u64 {
    pos
}

/// Compares `left` and `right` byte by byte. None when cancelled.
pub fn compare(left: &Path, right: &Path, cancel: &AtomicBool) -> Result<Option<HexDiff>> {
    let mut a = super::open_regular(left).ctx("open", left)?;
    let mut b = super::open_regular(right).ctx("open", right)?;
    let mut out = HexDiff {
        left_size: a.metadata().ctx("stat", left)?.len(),
        right_size: b.metadata().ctx("stat", right)?.len(),
        ..Default::default()
    };
    let short = out.left_size.min(out.right_size);
    const CHUNK: usize = 1 << 20;
    let (mut ba, mut bb) = (vec![0u8; CHUNK], vec![0u8; CHUNK]);
    let mut pos = 0u64;
    let mut open: Option<(u64, u64)> = None; // the range being built
    while pos < short {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let jumped = skip_shared_hole(&mut a, &mut b, pos, short);
        if jumped > pos {
            if let Some(r) = open.take() {
                push(&mut out, r);
            }
            pos = jumped;
            if pos >= short {
                break;
            }
        }
        let na = read_full(&mut a, &mut ba).ctx("read", left)?;
        let nb = read_full(&mut b, &mut bb).ctx("read", right)?;
        let n = na.min(nb);
        if ba[..n] != bb[..n] {
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

    #[test]
    fn an_insertion_shifts_every_byte_after_it_and_the_view_says_so_by_offset() {
        // Bytes are compared at the same offset (like `cmp`): inserting one byte at the front makes every byte differ
        // wherever it is not the same by chance, and the longer file has one byte with no counterpart.
        let a: Vec<u8> = (0..64u8).collect();
        let mut b = vec![0xffu8];
        b.extend_from_slice(&a);
        let r = diff(&a, &b);
        assert_eq!((r.left_size, r.right_size), (64, 65));
        assert_eq!(r.ranges, vec![(0, 65)]);
        assert_eq!(r.differing, 65);
    }

    #[test]
    fn empty_files_and_one_empty_side() {
        assert!(diff(b"", b"").ranges.is_empty());
        let r = diff(b"", b"abc");
        assert_eq!((r.ranges.clone(), r.differing), (vec![(0, 3)], 3));
        let r = diff(b"abc", b"");
        assert_eq!((r.ranges, r.differing), (vec![(0, 3)], 3));
    }

    #[test]
    fn sparse_files_with_distant_differences_are_streamed_without_keeping_them() {
        // Two 8 MiB files that are zero everywhere but at three far-apart places: only the ranges are kept.
        let d = tempfile::tempdir().unwrap();
        let (pa, pb) = (d.path().join("a"), d.path().join("b"));
        for p in [&pa, &pb] {
            std::fs::File::create(p).unwrap().set_len(8 << 20).unwrap();
        }
        let poke = |p: &Path, at: u64, byte: u8| {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
            f.seek(SeekFrom::Start(at)).unwrap();
            f.write_all(&[byte]).unwrap();
        };
        poke(&pb, 0, 1);
        poke(&pb, 4 << 20, 2);
        poke(&pb, (8 << 20) - 1, 3);
        let r = compare(&pa, &pb, &AtomicBool::new(false)).unwrap().unwrap();
        assert_eq!(
            r.ranges,
            vec![(0, 1), (4 << 20, (4 << 20) + 1), ((8 << 20) - 1, 8 << 20)]
        );
        assert_eq!(r.differing, 3);
        assert!(!r.truncated);
    }

    #[test]
    fn a_window_at_zero_at_the_end_past_it_and_at_an_unsafe_offset() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("f");
        std::fs::write(&p, b"0123456789").unwrap();
        assert_eq!(read_at(&p, 0, 4).unwrap(), b"0123");
        // Exactly at the end, and past it: nothing, not an error.
        assert!(read_at(&p, 10, 4).unwrap().is_empty());
        assert!(read_at(&p, 11, 4).unwrap().is_empty());
        // An offset no file can have is an error or nothing, never a panic or a huge allocation.
        assert!(read_at(&p, u64::MAX, 4).map_or(true, |bytes| bytes.is_empty()));
        assert!(read_at(&p, i64::MAX as u64, 4).map_or(true, |bytes| bytes.is_empty()));
        // A request for more than the most one read returns is cut, so a whole binary is never transferred at once.
        let big = d.path().join("big");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(MAX_READ * 4)
            .unwrap();
        assert_eq!(read_at(&big, 0, u64::MAX).unwrap().len() as u64, MAX_READ);
    }

    #[test]
    fn multi_gib_sparse_files_compare_via_hole_skipping() {
        let d = tempfile::tempdir().unwrap();
        let (pa, pb) = (d.path().join("a.bin"), d.path().join("b.bin"));
        let size = 4u64 << 30; // 4 GiB sparse
        for p in [&pa, &pb] {
            std::fs::File::create(p).unwrap().set_len(size).unwrap();
        }
        let poke = |p: &Path, at: u64, bytes: &[u8]| {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
            f.seek(SeekFrom::Start(at)).unwrap();
            f.write_all(bytes).unwrap();
        };
        let tail_off = size - 16;
        poke(&pa, 0, b"HEAD");
        poke(&pb, 0, b"HEAX");
        poke(&pa, tail_off, b"TAIL0000");
        poke(&pb, tail_off, b"TAIL0001");

        let r = compare(&pa, &pb, &AtomicBool::new(false)).unwrap().unwrap();
        assert_eq!(r.left_size, size);
        assert_eq!(r.right_size, size);
        assert_eq!(r.ranges, vec![(3, 4), (tail_off + 7, tail_off + 8)]);
        assert_eq!(r.differing, 2);
        assert!(!r.truncated);
        assert_eq!(read_at(&pb, tail_off, 8).unwrap(), b"TAIL0001");
    }
}
