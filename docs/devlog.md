# Development log

## 2026-10-31
- **Scope and method.** Clean-room policy written. OneCommander binaries are never decompiled. Sources: public CHM manual (63 pages), release notes, product pages, and user-facing readmes in the v4 ZIP.
- **Observation.** The original is Windows-only. Running it under Wine was declined by the maintainer (reliability). Live checks are therefore listed as OBS requests for a Windows machine.
- **Stack decision** (maintainer): Tauri v2 + React/TypeScript + Rust core, replacing an earlier Avalonia proposal.
- **Built `gall-core`:** listing, drives, watcher with storm guard, wildcard search, task manager with conflict policies and undo journal, history/tabs/session, sort/filter/age, preview metadata, JSON store with backup recovery, SQLite prefs/history/tags/destinations, templates, notes, clipboard naming, terminal launch. 50 unit tests.
- **Built the Tauri adapter** (about 55 typed commands, plus events `fs-changed`, `task`, `search-batch`, `search-done`, `second-instance`) and CLI parsing.
- **Built the React UI:** all three layouts, Miller columns, tabs, virtualised folder pane with five views, details/preview, sidebar, overlays (path edit, templates, notes, tags, properties, preview window, search, settings), notifications and progress. 11 UI logic tests.
- **First launch verified** with isolated config and test data (screenshot).
- **Licence audit:** all dependencies MIT/Apache-2.0, except five unmodified MPL-2.0 crates (compatible).
- **Next:** interactive QA pass, batch tools (X-04, needs OBS-04), multiple/named windows, a thumbnail cache in Rust, drag-out to other apps, "Open with…".

## 2026-10-31 (later): drag & drop fix
- **Report** (maintainer): dragging a folder to move or copy it does nothing.
- **Backend ruled out:** a new unit test moves and copies a nested folder tree; it passes.
- **Causes found in the UI (WebKitGTK specifics):**
  1. `user-select: none` blocks starting drags unless `-webkit-user-drag: element` is set.
  2. Custom MIME types are not carried through GTK drag & drop. Tab and favourite drags carried *only* custom data, so they had no data and might not start.
  3. Alt-drag requested `link`, which `effectAllowed` did not permit.
- **Fix:** internal drag payload kept in memory (`state/dnd.ts`); every drag also sets `text/plain` (+ `text/uri-list` for files); CSS rule added; `effectAllowed = "all"`; dropping a folder into itself is refused with a message.
- **Diagnostics:** UI errors and empty drops are now written to the app log (`ui_log` command).
- **Status:** still needs an interactive check on the maintainer's machine; this environment cannot drive Wayland input.

## 2026-10-31 (later): sticky modifiers during drag
- **Report:** after pressing Ctrl during a drag, copy stays selected until Shift is pressed. The maintainer wants behaviour like the reference product: a modifier only while held, otherwise a configurable default.
- **Cause (hypothesis, consistent with the report):** WebKitGTK fills `event.ctrlKey/shiftKey` during DnD from the negotiated GTK drag action, not the physical keys.
- **Fix:** new `modifiers` command reads `gdk::Keymap::modifier_state()` on the main thread. The UI polls it every 60 ms while dragging, updates the hint even without pointer motion, and re-reads it at drop time. Falls back to event flags where GDK is unavailable.
- **New setting** `dragBehavior`: consistent (move) | copy | system (by drive) | ask. 4 new UI tests for the decision logic.
- **Needs interactive confirmation** on KDE Wayland.

## 2026-10-31 (later): automated UI testing
- There is no WebKitWebDriver and no input injection on Wayland, so I built a debug-only self-test (`ui/src/selftest.ts`, `GALL_SELFTEST=1`). It drives the real UI with DOM events, takes Spectacle screenshots and runs in a private D-Bus session with temp config.
- Run 1: 44/48. The 3 failures in the test itself were fixed. One real bug: the drag hint lost event modifiers when GDK state was unknown.
- Screenshot review found 4 visual bugs: clipped date column, misaligned sort arrows, mangled RTL path in search, tiny colour input. All fixed, with regression checks for clipping and alignment.
- Run 3: 50/50 PASS.
