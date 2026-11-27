//! Golden parity: the portable adapter (what the browser worker runs) and the desktop comparison
//! must give the same changed ranges, moved blocks and counts for the same bytes.

use gall_compare::adapter::{compare, Answer, Options, Whitespace};
use gall_core::fs::diff::{diff_text, DiffOptions};

fn never() -> bool {
    false
}

fn stay(tag: &str) -> String {
    (0..12)
        .map(|k| format!("stay_{tag}_statement_number_{k}();\n"))
        .collect()
}

/// Inputs that cover edits, inserts, deletes, a moved block, repeated lines and CRLF.
fn cases() -> Vec<(String, String)> {
    let f = "fn helper(x: u32) -> u32 {\n    let y = x * 2;\n    y + 1\n}\n";
    let s = stay("p");
    let long: String = (0..60).map(|i| format!("line {i}\n")).collect();
    vec![
        (String::new(), String::new()),
        ("a\n".into(), String::new()),
        (String::new(), "a\nb\n".into()),
        ("a\nb\nc\n".into(), "a\nx\nc\n".into()),
        ("a\nb\nc".into(), "a\nb\nc\n".into()),
        (
            long.clone(),
            long.replace("line 7\n", "seven\n")
                .replace("line 40\n", "forty\nforty-one\n"),
        ),
        (format!("{f}{s}"), format!("{s}{f}")),
        ("x\nx\nx\nx\n".into(), "x\nx\n".into()),
        ("a\r\nb\r\n".into(), "a\nb\n".into()),
        ("ž\n日本語\n".into(), "ž\n日本\n".into()),
    ]
}

/// The same settings for both: (adapter options, desktop options).
fn option_sets() -> Vec<(Options, DiffOptions)> {
    use gall_core::fs::diff::Whitespace as W;
    let desktop = |ignore_eol, whitespace, ignore_case, minimal| DiffOptions {
        ignore_eol,
        whitespace,
        ignore_case,
        minimal,
        structural: false,
        ..DiffOptions::default()
    };
    vec![
        (Options::default(), desktop(false, W::None, false, false)),
        (
            Options {
                ignore_eol: true,
                ..Options::default()
            },
            desktop(true, W::None, false, false),
        ),
        (
            Options {
                whitespace: Whitespace::All,
                ignore_case: true,
                ..Options::default()
            },
            desktop(false, W::All, true, false),
        ),
        (
            Options {
                whitespace: Whitespace::Leading,
                ignore_eol: true,
                ..Options::default()
            },
            desktop(true, W::Leading, false, false),
        ),
        (
            Options {
                whitespace: Whitespace::Trailing,
                ..Options::default()
            },
            desktop(false, W::Trailing, false, false),
        ),
        (
            Options {
                minimal: true,
                ..Options::default()
            },
            desktop(false, W::None, false, true),
        ),
    ]
}

#[test]
fn adapter_and_desktop_agree() {
    for (ours_opts, desktop_opts) in option_sets() {
        for (a, b) in cases() {
            let desktop = diff_text(&a, &b, desktop_opts.clone());
            let Answer::Text {
                ops,
                moves,
                added,
                removed,
                identical,
            } = compare(a.as_bytes(), b.as_bytes(), &ours_opts, &never)
            else {
                panic!("text expected for {a:?} / {b:?}");
            };
            let theirs: Vec<_> = desktop
                .ops
                .iter()
                .map(|o| (o.tag, o.a, o.a_len, o.b, o.b_len))
                .collect();
            let ours: Vec<_> = ops
                .iter()
                .map(|o| (o.tag, o.a, o.a_len, o.b, o.b_len))
                .collect();
            assert_eq!(ours, theirs, "ops of {a:?} / {b:?} with {ours_opts:?}");
            assert_eq!(
                moves, desktop.moves,
                "moves of {a:?} / {b:?} with {ours_opts:?}"
            );
            assert_eq!((added, removed), (desktop.added, desktop.removed));
            assert_eq!(identical, desktop.added == 0 && desktop.removed == 0);
        }
    }
}

#[test]
fn binary_classification_matches() {
    // The desktop reads these as binary (a NUL byte, invalid UTF-8): so does the adapter.
    for bytes in [&b"a\0b"[..], &[0xff, 0xfe, 0x00][..]] {
        assert_eq!(
            compare(bytes, b"text", &Options::default(), &never),
            Answer::Binary
        );
    }
}

#[test]
fn edited_moves_match_the_desktop_option() {
    let block = "fn compute_total(items: &[Item]) -> u64 {\n    let mut sum = 0;\n    for item in items {\n        sum += item.price_in_cents();\n    }\n    sum\n}\n";
    let s = stay("q");
    let a = format!("{block}{s}end_of_file_marker();\n");
    let b = format!(
        "{s}{}end_of_file_marker();\n",
        block.replace("price_in_cents", "price_with_tax")
    );
    let desktop = diff_text(
        &a,
        &b,
        DiffOptions {
            structural: false,
            moves_edited: true,
            ..DiffOptions::default()
        },
    );
    let Answer::Text { moves, .. } = compare(
        a.as_bytes(),
        b.as_bytes(),
        &Options {
            moves_edited: true,
            ..Options::default()
        },
        &never,
    ) else {
        panic!("text expected")
    };
    assert_eq!(moves, desktop.moves);
    assert!(moves.iter().any(|m| m.edited));
}
