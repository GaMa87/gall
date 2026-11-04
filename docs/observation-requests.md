# Observation requests (black-box checks on Windows)

The maintainer runs these on a licensed OneCommander install with **disposable test
folders** and records the results as `OBS-nn` (notes or screenshots of observable
behaviour only).

These checks ask **how features behave**, not how they look or what they say. Gall
does not copy the reference product's wording, labels, icons or visual design; dialogs,
messages and settings are written and laid out independently.

| ID | Question | Why |
|----|----------|-----|
| OBS-01 | Drag feedback: when the move/copy indication changes (modifier keys, target drive) and whether it updates without pointer motion | O-11 behaviour |
| OBS-02 | Which columns hide first as a browser narrows in the Default view (order and approximate widths) | F-01 removal order |
| OBS-03 | Ctrl+Shift+T: reopen closed tab, or new tab from clipboard path? (the manual says both) | T-03 / T-09 conflict |
| OBS-04 | Batch rename: which kinds of renaming are supported (counters, find/replace, RegEx, case changes) and whether a live preview is shown | X-04 scope, next milestone |
| OBS-05 | Age colour scale: which colour corresponds to which age bucket by default | F-05 |
| OBS-06 | Thumbs+Details: second-line content for image, PDF, docx, txt | V-01 |
| OBS-07 | Rename popup: position (in place or dialog), and whether Enter on an unchanged name closes it | O-04 |
| OBS-08 | Name conflicts on copy/move: which outcomes are possible (skip, overwrite, keep both, apply to all) and whether the queue pauses | O-16 |
| OBS-09 | Undo history UI (v4): is there a list, or only Ctrl+Z? | O-15 |
| OBS-10 | Settings coverage: which behaviours are configurable at all (a feature checklist, not labels or layout) | C-01 completeness |
