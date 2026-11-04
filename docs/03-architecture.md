# Architecture (Phase 4)

## Decision summary

| Topic | Decision | Reason |
|-------|----------|--------|
| Shell | **Tauri v2** | Small native binary and system webview (WebKitGTK, WebView2, WKWebView). Cross-platform. |
| UI | **React 19 + TypeScript + Vite** | Mature component model. Virtualised lists in the DOM handle folders with 100k+ entries. |
| Core | **Rust** workspace crate `gall-core`, with no Tauri dependency | Business logic is unit-testable without a UI and reusable for a CLI. |
| IPC | Typed Tauri commands (request/response) plus events (push) | One `api.ts` mirrors the Rust DTOs, so the commands are typed on both sides. |
| Storage | SQLite (`rusqlite`, bundled) for per-folder preferences, history and tags. JSON for settings, favourites and session. | Per-folder data needs indexed lookups with ancestor inheritance. User-editable config stays human-readable. |
| Settings safety | Atomic write (temp file + rename) that keeps `settings.backup.json`. Corrupt file → backup → defaults. | Matches observed behaviour C-02 and improves on it with atomic writes. |
| File ops | Task manager: a queue of jobs on worker threads, with progress events, cancellation, conflict policy and an **undo journal** | O-15, O-16 |
| Watcher | `notify` with debouncing, one watcher per visible folder, plus a storm detector | F-11 |
| Trash | `trash` crate (freedesktop Trash on Linux, Recycle Bin on Windows) | O-06 |

## Layers

```
┌─────────────────────────── ui/ (React + TS) ────────────────────────────┐
│ App ─ Sidebar(Drives, Favorites, History, MainToolbar)                   │
│     ─ Browser×2 ─ TabBar ─ NavPane(Miller | Breadcrumbs) ─ FolderPane    │
│                                                   ─ DetailsPane(Preview) │
│     ─ Overlays: PathEdit, Rename, NewItem, Templates, Search, Settings,  │
│                 PreviewWindow, ContextMenu, Notifications                │
│ state: store.ts (windows → browsers → tabs), keymap.ts (rebindable)      │
└──────────────── api.ts  ⇅  invoke("cmd") / listen("event") ──────────────┘
┌──────────────────────── src-tauri/ (thin adapter) ───────────────────────┐
│ commands.rs (DTO in/out, error → string)   events: fs-changed, task-*    │
│ AppState { Core, watchers, task manager handle }                         │
└───────────────────────────────────┬──────────────────────────────────────┘
┌───────────────────── crates/gall-core (pure Rust) ──────────────────┐
│ fs::      listing, entry, drives, watcher, search, ops(TaskManager,undo) │
│ nav::     history, tabs, path ancestors (Miller), path formats           │
│ view::    sort, quick filter, age heat                                   │
│ preview:: classify, text excerpt, image dims, archive list               │
│ storage:: settings (json), favorites (json), folder prefs (sqlite)       │
│ util::    templates, notes (.2do/.2dx), clipboard naming                 │
└──────────────────────────────────────────────────────────────────────────┘
```

## Data flow examples

- **Navigate:** UI → `list_dir(path, show_hidden)` → `Vec<EntryDto>`. Sorting and filtering run in the UI for responsiveness; the same algorithms exist in core and are tested there. `get_folder_prefs(path)` returns the inherited sort/view. `watch(path)` → `fs-changed {path}` events → UI reloads.
- **Copy:** UI → `start_task({kind:"copy", sources, dest})` → id. The core worker emits `task-progress` and `task-done`. Completed jobs push inverse operations onto the undo journal. `undo()` pops and runs the inverse.
- **Preview:** `preview_info(path)` → kind, metadata, text excerpt, archive listing. Images, video and audio load through Tauri's asset protocol (`convertFileSrc`). PDFs render in the webview through pdf.js (planned).

## Observed vs hypothesis

All behaviour requirements come from `01-feature-inventory.md` ([Observed]).
The internal design here is **our own**. Nothing about OneCommander's internals is
known or assumed, beyond the dependency names listed in the inventory.

## Platform mapping

| Windows concept (reference) | Gall |
|-----------------------------|-----------|
| Drive letters | Mount points (Linux/macOS), drive letters (Windows) through `sysinfo` |
| Recycle Bin | `trash` crate |
| Shell context menu | Built-in menu plus user actions (`actions.json`) |
| CMD/PowerShell F10 | Configurable terminal command (auto-detected: `$TERMINAL`, gnome-terminal, konsole, xterm) |
| Hidden attribute | Leading dot (Unix) or the hidden attribute (Windows) |
| `%USERPROFILE%` | `~`, `$HOME`, `%VAR%`, `$VAR` expansion |
