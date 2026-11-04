//! Quick filter (F-04): one character = "starts with", two or more = "contains",
//! a leading '.' filters by extension. Case-insensitive.

pub fn quick_filter_matches(name: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    // Typed without accents, "porocilo" also finds "Poročilo" (see `fuzzy::fold_char`).
    let fold = query.chars().all(|c| crate::fs::fuzzy::fold_char(c) == c);
    let n: String = if fold {
        name.chars()
            .map(crate::fs::fuzzy::fold_char)
            .collect::<String>()
            .to_lowercase()
    } else {
        name.to_lowercase()
    };
    let q = query.to_lowercase();
    if let Some(ext) = q.strip_prefix('.') {
        if ext.is_empty() {
            return n.contains('.');
        }
        return match n.rfind('.') {
            Some(i) if i > 0 => n[i + 1..].starts_with(ext),
            _ => false,
        };
    }
    if q.chars().count() == 1 {
        n.starts_with(&q)
    } else {
        n.contains(&q)
    }
}

#[cfg(test)]
mod tests {
    use super::quick_filter_matches as m;

    #[test]
    fn accents_fold_unless_typed() {
        assert!(m("Poročilo.txt", "poroc"));
        assert!(m("Čebela.txt", "c"), "one letter: starts with");
        assert!(!m("porocilo.txt", "poroč"));
    }

    #[test]
    fn rules() {
        assert!(m("Report.pdf", "r"));
        assert!(!m("Report.pdf", "p"), "single letter = starts with");
        assert!(m("Report.pdf", "po"), "two letters = contains");
        assert!(m("Report.pdf", ".pdf"));
        assert!(m("Report.pdf", ".pd"));
        assert!(!m("Report.pdf.txt", ".pdf"));
        assert!(!m(".bashrc", ".bash"), "dot-files have no extension");
        assert!(m("anything", ""));
    }
}
