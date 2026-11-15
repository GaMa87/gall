//! Unicode normalization for comparing names. macOS (HFS+, some archives and tools) hands
//! out names decomposed (NFD: "c" + U+030C), while a user types or Git prints them
//! composed (NFC: "č"). Names are compared and matched in NFC; files are never renamed.

use std::borrow::Cow;
use unicode_normalization::{is_nfc_quick, IsNormalized, UnicodeNormalization};

/// `s` in NFC. ASCII and text that is already composed are returned without copying.
pub fn nfc(s: &str) -> Cow<'_, str> {
    if s.is_ascii() || is_nfc_quick(s.chars()) == IsNormalized::Yes {
        return Cow::Borrowed(s);
    }
    Cow::Owned(s.nfc().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decomposed_names_compose() {
        let nfd = "Poroc\u{30c}ilo";
        assert_ne!(nfd, "Poročilo");
        assert_eq!(nfc(nfd), "Poročilo");
        assert!(matches!(nfc("plain"), Cow::Borrowed(_)));
        assert!(matches!(nfc("Poročilo"), Cow::Borrowed(_)));
    }
}
