use gall_core::compare::{InputError, LineIndex, MAX_INPUT_BYTES, MAX_INPUT_LINES, MAX_LINE_BYTES};
use std::sync::atomic::AtomicBool;

#[test]
fn indexed_text_preserves_bom_utf8_and_line_endings() {
    let cancel = AtomicBool::new(false);
    let index = LineIndex::new("\u{feff}Črka\r\n\nlast".as_bytes(), &cancel).unwrap();
    assert_eq!(index.len(), 3);
    assert_eq!(index.raw(0), Some("\u{feff}Črka\r"));
    assert_eq!(index.display(0), Some("\u{feff}Črka"));
    assert_eq!(index.display(1), Some(""));
    assert_eq!(index.display(2), Some("last"));
    assert_eq!(index.raw(3), None);
    assert!(index.crlf());
    assert!(!index.final_newline());

    let empty = LineIndex::new(b"", &cancel).unwrap();
    assert!(empty.is_empty());
    assert!(empty.final_newline());
    let insertion = LineIndex::new(b"\n", &cancel).unwrap();
    assert_eq!(insertion.len(), 1);
    assert_eq!(insertion.raw(0), Some(""));
    assert!(insertion.final_newline());
}

#[test]
fn indexed_text_has_explicit_binary_cancel_and_size_limits() {
    let cancel = AtomicBool::new(false);
    for bytes in [&b"a\0b"[..], &[0xff][..]] {
        assert_eq!(
            LineIndex::new(bytes, &cancel).unwrap_err(),
            InputError::Binary
        );
    }
    assert_eq!(
        LineIndex::new(b"text", &AtomicBool::new(true)).unwrap_err(),
        InputError::Cancelled
    );
    assert_eq!(
        LineIndex::new(&vec![b'x'; MAX_INPUT_BYTES + 1], &cancel).unwrap_err(),
        InputError::TooManyBytes
    );
    assert_eq!(
        LineIndex::new(&vec![b'\n'; MAX_INPUT_LINES + 1], &cancel).unwrap_err(),
        InputError::TooManyLines
    );
    let long_line = vec![b'x'; MAX_LINE_BYTES + 1];
    assert_eq!(
        LineIndex::new(&long_line, &cancel).unwrap_err(),
        InputError::LineTooLong
    );
    assert_eq!(
        LineIndex::new(&long_line[..MAX_LINE_BYTES], &cancel)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn comparison_pages_preserve_legacy_ops_and_stable_insertion_ids() {
    use gall_core::compare::{CompareSide, Comparison};
    use gall_core::fs::diff::{diff_text, DiffOptions};

    let cancel = AtomicBool::new(false);
    for minimal in [false, true] {
        let options = DiffOptions {
            minimal,
            ignore_eol: true,
            ..Default::default()
        };
        let left = "\u{feff}Črka\r\nlast";
        let right = "\u{feff}Črka\r\ninserted\r\nlast";
        let comparison = Comparison::new(
            left.as_bytes(),
            right.as_bytes(),
            &[],
            options.clone(),
            &cancel,
        )
        .unwrap();
        let legacy = diff_text(left, right, options);
        let op = &comparison.hunk_page(1, 1).unwrap().items[0];
        assert_eq!(op.op.tag, "insert");
        assert_eq!(op.id, "insert:1:0:1:1");
        assert_eq!(comparison.hunk_page(0, 256).unwrap().items[1].id, op.id);
        assert_eq!(
            serde_json::to_value(&comparison.diff().ops).unwrap(),
            serde_json::to_value(&legacy.ops).unwrap()
        );
        assert_eq!(
            comparison
                .line_page(CompareSide::Right, 1, 2)
                .unwrap()
                .items,
            ["inserted", "last"]
        );
    }
}

#[test]
fn range_response_budget_counts_escaped_bytes_and_keeps_making_progress() {
    use gall_core::compare::{CompareSide, Comparison, MAX_PAGE_BYTES};
    use gall_core::fs::diff::DiffOptions;

    let line = "\u{1}".repeat(MAX_LINE_BYTES);
    let input = format!("{line}\n{line}\n{line}");
    let cancel = AtomicBool::new(false);
    let comparison = Comparison::new(
        input.as_bytes(),
        input.as_bytes(),
        &[],
        DiffOptions::default(),
        &cancel,
    )
    .unwrap();

    let mut next = Some(0);
    let mut delivered = 0;
    while let Some(start) = next {
        let page = comparison
            .line_page(CompareSide::Left, start, usize::MAX)
            .unwrap();
        assert_eq!(page.items, [line.as_str()]);
        assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_PAGE_BYTES);
        delivered += page.items.len();
        next = page.next;
    }
    assert_eq!(delivered, 3);
}

#[test]
fn paged_adapter_keeps_binary_verdicts_and_reports_oversized_files() {
    use gall_core::fs::diff::{compare_paged, DiffOptions, DiffSource};
    use std::fs;

    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    fs::write(&a, [0, 2, 3]).unwrap();
    fs::write(&b, [0, 4, 3]).unwrap();
    let compare = || {
        compare_paged(
            &DiffSource::File { path: a.clone() },
            &DiffSource::File { path: b.clone() },
            DiffOptions::default(),
            &AtomicBool::new(false),
        )
        .unwrap()
    };
    let binary = compare().binary.unwrap();
    assert!(!binary.identical);
    assert_eq!(binary.first_difference, Some(1));

    fs::File::create(&a)
        .unwrap()
        .set_len(MAX_INPUT_BYTES as u64 + 1)
        .unwrap();
    let result = compare();
    assert_eq!(result.limit, Some(InputError::TooManyBytes));
    assert!(result.text.is_none());
}

#[test]
fn long_replacement_skips_inline_enrichment_and_preserves_line_ops() {
    use gall_core::compare::Comparison;
    use gall_core::fs::diff::DiffOptions;

    let left = "x!".repeat(10_000);
    let right = "y?".repeat(10_000);
    let comparison = Comparison::new(
        left.as_bytes(),
        right.as_bytes(),
        &["source.rs"],
        DiffOptions {
            structural: true,
            ..Default::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!comparison.is_detailed());
    assert!(comparison.diff().pairs.is_empty());
    assert!(comparison.diff().left_marks.is_empty());
    assert_eq!(
        comparison.hunk_page(0, 1).unwrap().items[0].op.tag,
        "replace"
    );
}

#[test]
fn exact_myers_has_an_explicit_work_limit() {
    use gall_core::compare::Comparison;
    use gall_core::fs::diff::DiffOptions;

    let text = "line\n".repeat(5_001);
    let error = Comparison::new(
        text.as_bytes(),
        text.as_bytes(),
        &[],
        DiffOptions {
            minimal: true,
            ..Default::default()
        },
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert_eq!(error, InputError::MinimalWorkLimit);
}
