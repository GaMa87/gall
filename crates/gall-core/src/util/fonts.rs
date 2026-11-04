//! Installed font families, for the font picker in Settings (A-02). Read with fontdb on
//! every OS: the system font folders on Windows and macOS, fontconfig's on Linux.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FontFamily {
    pub name: String,
    /// Every face of the family has fixed-width glyphs (a code font).
    pub monospace: bool,
    /// The family has a bold and an italic face (else the browser fakes them).
    pub bold: bool,
    pub italic: bool,
}

/// The installed families, sorted by name; scanning reads the header of every font file,
/// so call it on demand (a moment on a system with many fonts).
pub fn families() -> Vec<FontFamily> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut by: std::collections::BTreeMap<String, FontFamily> = Default::default();
    for face in db.faces() {
        // The first name is the family name in English (or the font's own language).
        let Some((name, _)) = face.families.first() else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || name.starts_with('.') {
            continue; // macOS system-only faces (".SF NS")
        }
        let e = by.entry(name.to_lowercase()).or_insert_with(|| FontFamily {
            name: name.to_string(),
            monospace: true,
            bold: false,
            italic: false,
        });
        e.monospace &= face.monospaced;
        e.bold |= face.weight.0 >= 600;
        e.italic |= face.style != fontdb::Style::Normal;
    }
    by.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Any desktop has fonts; the list is sorted, without duplicates.
    #[test]
    fn lists_installed_families() {
        let f = families();
        if f.is_empty() {
            eprintln!("no fonts on this system (a bare CI container): skipped");
            return;
        }
        let names: Vec<String> = f.iter().map(|x| x.name.to_lowercase()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(names, sorted);
        #[cfg(windows)]
        {
            let arial = f
                .iter()
                .find(|x| x.name == "Arial")
                .expect("Arial on Windows");
            assert!(!arial.monospace && arial.bold && arial.italic);
            assert!(f.iter().any(|x| x.name == "Consolas" && x.monospace));
        }
    }
}
