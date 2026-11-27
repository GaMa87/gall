//! Golden fixtures of the syntax-tree comparison: every case of `tests/semantic/` is a pair of files
//! (`<case>.left.<ext>`, `<case>.right.<ext>`) and a hand-written expectation (`<case>.expect`): `equivalent`,
//! `unknown`, or `different: <kinds, sorted>`. A case that fails is either a defect of the comparison or a wrong
//! expectation: decide which before changing either side, and say so in the commit.
use gall_core::semantic_compare::{compare, language_of, ChangeKind, Verdict};
use std::path::Path;

fn kind_name(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Rename => "rename",
        ChangeKind::Signature => "signature",
        ChangeKind::Moved => "moved",
        ChangeKind::Behavior => "behavior",
        ChangeKind::Added => "added",
        ChangeKind::Removed => "removed",
    }
}

fn describe(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Equivalent { .. } => "equivalent".into(),
        Verdict::Unknown { .. } => "unknown".into(),
        Verdict::Cancelled => "cancelled".into(),
        Verdict::Different { changes } => {
            let mut kinds: Vec<&str> = changes.iter().map(|c| kind_name(c.kind)).collect();
            kinds.sort();
            kinds.dedup();
            format!("different: {}", kinds.join(", "))
        }
    }
}

#[test]
fn every_golden_case_has_the_expected_verdict() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/semantic");
    let mut cases: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".expect").map(str::to_owned))
        })
        .collect();
    cases.sort();
    assert!(cases.len() >= 15, "the fixtures are missing: {cases:?}");
    let mut wrong = Vec::new();
    for case in &cases {
        let find = |side: &str| {
            std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .find(|n| n.starts_with(&format!("{case}.{side}.")))
                .unwrap_or_else(|| panic!("{case}: no {side} file"))
        };
        let (left_name, right_name) = (find("left"), find("right"));
        let left = std::fs::read_to_string(dir.join(&left_name)).unwrap();
        let right = std::fs::read_to_string(dir.join(&right_name)).unwrap();
        let expected = std::fs::read_to_string(dir.join(format!("{case}.expect"))).unwrap();
        let verdict = compare(&left, &right, language_of(&right_name), &|| false);
        let mut lines = expected.lines().map(str::trim).filter(|l| !l.is_empty());
        let expected_verdict = lines.next().unwrap_or("");
        let got = describe(&verdict);
        if got != expected_verdict {
            wrong.push(format!(
                "{case}: expected `{expected_verdict}`, got `{got}`"
            ));
        } else if let Some(rest) = lines.next().and_then(|l| l.strip_prefix("names:")) {
            let expected_names = rest.trim();
            let got_names = match &verdict {
                Verdict::Different { changes } => {
                    let mut names: Vec<&str> =
                        changes.iter().filter_map(|c| c.name.as_deref()).collect();
                    names.sort();
                    names.join(", ")
                }
                _ => String::new(),
            };
            if got_names != expected_names {
                wrong.push(format!(
                    "{case}: expected names `{expected_names}`, got `{got_names}`"
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
}
