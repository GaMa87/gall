//! The Windows file clipboard formats, as plain bytes so they are testable everywhere:
//! `CF_HDROP` (a `DROPFILES` header followed by NUL-separated UTF-16 paths) and the
//! `Preferred DropEffect` DWORD that tells Explorer whether the files are copied or cut.

/// `DROPEFFECT_COPY`.
pub const DROPEFFECT_COPY: u32 = 1;
/// `DROPEFFECT_MOVE`.
pub const DROPEFFECT_MOVE: u32 = 2;

/// Size of the `DROPFILES` header: `pFiles` (u32), `pt` (2 × i32), `fNC` (u32), `fWide` (u32).
const HEADER: usize = 20;

/// Drops the `\\?\` prefix Windows APIs add (`\\?\C:\x` → `C:\x`, `\\?\UNC\h\s` → `\\h\s`):
/// Explorer and most programs do not expect it in a drop list.
fn plain_path(p: &str) -> String {
    if let Some(rest) = p.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = p.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        p.to_string()
    }
}

/// The `CF_HDROP` payload for `paths`: wide characters, each path ended by NUL and the list by
/// an extra NUL. Paths with an embedded NUL cannot be represented and are skipped.
pub fn build_hdrop(paths: &[String]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + paths.len() * 64);
    out.extend_from_slice(&(HEADER as u32).to_le_bytes()); // pFiles
    out.extend_from_slice(&[0u8; 8]); // pt
    out.extend_from_slice(&0u32.to_le_bytes()); // fNC
    out.extend_from_slice(&1u32.to_le_bytes()); // fWide
    for p in paths.iter().filter(|p| !p.contains('\0')) {
        for unit in plain_path(p).encode_utf16() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0]);
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// The paths in a `CF_HDROP` payload (wide or ANSI list); empty when it is malformed.
pub fn parse_hdrop(data: &[u8]) -> Vec<String> {
    if data.len() < HEADER {
        return Vec::new();
    }
    let start = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let wide = u32::from_le_bytes(data[16..20].try_into().unwrap()) != 0;
    if start < HEADER || start > data.len() {
        return Vec::new();
    }
    let body = &data[start..];
    let mut out = Vec::new();
    if wide {
        let units: Vec<u16> = body
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        for part in units.split(|&u| u == 0) {
            if part.is_empty() {
                // The empty entry that ends the list (or a stray NUL).
                if out.is_empty() {
                    continue;
                }
                break;
            }
            out.push(String::from_utf16_lossy(part));
        }
    } else {
        for part in body.split(|&b| b == 0) {
            if part.is_empty() {
                if out.is_empty() {
                    continue;
                }
                break;
            }
            out.push(String::from_utf8_lossy(part).into_owned());
        }
    }
    out
}

/// The `Preferred DropEffect` value for a copy (`false`) or a cut (`true`).
pub fn drop_effect(cut: bool) -> [u8; 4] {
    (if cut {
        DROPEFFECT_MOVE
    } else {
        DROPEFFECT_COPY
    })
    .to_le_bytes()
}

/// Whether a `Preferred DropEffect` payload asks for a move. A missing or short value is a copy.
pub fn is_cut(effect: Option<&[u8]>) -> bool {
    effect
        .and_then(|b| b.get(..4))
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .is_some_and(|v| v & DROPEFFECT_MOVE != 0 && v & DROPEFFECT_COPY == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_unicode_and_long_paths() {
        let long = format!(r"C:\{}\datoteka.txt", "a".repeat(300));
        let paths = vec![
            r"C:\Users\Matej\Čšž dokument 日本語 😀.txt".to_string(),
            r"\\server\share\mapa\b.txt".to_string(),
            long,
        ];
        assert_eq!(parse_hdrop(&build_hdrop(&paths)), paths);
    }

    #[test]
    fn header_is_a_wide_dropfiles() {
        let d = build_hdrop(&["C:\\a".to_string()]);
        assert_eq!(&d[0..4], &20u32.to_le_bytes());
        assert_eq!(&d[16..20], &1u32.to_le_bytes());
        // "C:\a" + NUL + final NUL, two bytes each.
        assert_eq!(d.len(), 20 + 2 * (4 + 1 + 1));
        assert_eq!(&d[d.len() - 4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn verbatim_prefixes_are_dropped() {
        let d = build_hdrop(&[r"\\?\C:\x\y".to_string(), r"\\?\UNC\h\s\z".to_string()]);
        assert_eq!(parse_hdrop(&d), [r"C:\x\y", r"\\h\s\z"]);
    }

    #[test]
    fn empty_list_and_malformed_data() {
        assert!(parse_hdrop(&build_hdrop(&[])).is_empty());
        assert!(parse_hdrop(&[1, 2, 3]).is_empty());
        let mut bad = build_hdrop(&["C:\\a".to_string()]);
        bad[0..4].copy_from_slice(&9999u32.to_le_bytes());
        assert!(parse_hdrop(&bad).is_empty());
    }

    #[test]
    fn reads_an_ansi_list() {
        let mut d = vec![0u8; 20];
        d[0..4].copy_from_slice(&20u32.to_le_bytes());
        d.extend_from_slice(b"C:\\a.txt\0D:\\b.txt\0\0");
        assert_eq!(parse_hdrop(&d), ["C:\\a.txt", "D:\\b.txt"]);
    }

    #[test]
    fn paths_with_a_nul_are_skipped() {
        let d = build_hdrop(&["C:\\a\0b".to_string(), "C:\\ok".to_string()]);
        assert_eq!(parse_hdrop(&d), ["C:\\ok"]);
    }

    #[test]
    fn drop_effect_distinguishes_cut_from_copy() {
        assert!(is_cut(Some(&drop_effect(true))));
        assert!(!is_cut(Some(&drop_effect(false))));
        assert!(!is_cut(None));
        assert!(!is_cut(Some(&[2, 0])));
        // Copy and move both offered (what a drag gives): treated as a copy.
        assert!(!is_cut(Some(&3u32.to_le_bytes())));
    }
}
