# Feature compatibility matrix (Phase 6)

Status: ✅ implemented · 🟡 partial · ❌ not yet · ➖ not applicable on this platform.
**Verified** describes how it was checked: `unit` = automated test, `build` = compiles and
renders (screenshot 2026-10-31), `manual` = interactive check pending. Comparison with
the original is still against the manual only (`M:`), not against live observation (`OBS:`).

| ID | Feature | Class | Status | Verified | Notes |
|----|---------|-------|--------|----------|-------|
| W-01 | Main window: sidebar + browsers | E | ✅ | build | |
| W-02 | Standard layout | E | ✅ | build | |
| W-03 | Columns layout | E | ✅ | manual | |
| W-04 | Classic layout | I | ✅ | manual | |
| W-05 | Layout switch | E | ✅ | manual | Main toolbar + Ctrl+Shift+L |
| W-06 | Collapse browser by divider | I | ✅ | manual | Plus auto single-browser when narrower than 760 px |
| W-07 | Divider arrows | I | 🟡 | manual | Browser divider only; per-pane arrows missing |
| W-08 | Main toolbar | E | 🟡 | build | Settings, About, Layout, Search, Undo. Batch tools and New Window missing |
| W-09 | Notifications + history | I | ✅ | manual | `~` toggles the log |
| W-10/11 | Multiple and named windows | I | ❌ | | Single window; session is persisted |
| W-12 | Tray / background | O | ❌ | | |
| W-13 | Global hotkeys | O | ❌ | | Wayland restricts global shortcuts; needs a portal |
| W-14 | Folder background image | O | ✅ | manual | |
| N-01 | Miller columns | E | ✅ | build | |
| N-02 | Drives column | E | ✅ | build | |
| N-03 | Compression + hover expand | I | ✅ | manual | Expand delay is configurable. F8 toggle missing |
| N-04 | Breadcrumbs ⇄ columns | E | ✅ | manual | `` ` `` key, button, drag divider up |
| N-05 | Keyboard navigation | E | 🟡 | manual | In the file list. Tab between panes is not implemented |
| N-06 | Click behaviour | E | ✅ | manual | Single-click option |
| N-07 | Per-tab history | E | ✅ | unit | Alt+←/→, mouse buttons 4/5 |
| N-08 | Up button | E | ✅ | manual | Reselects the folder you came from |
| N-09 | Path edit dropdown | E | ✅ | unit (complete) | Typing `explorer` is ➖ |
| N-10 | Path copy formats | I | ✅ | unit | `\\?\` and 8.3 are ➖. `file://` URI added |
| N-11 | Recent destinations | O | 🟡 | unit | Stored; no UI list yet |
| T-01…T-08 | Tabs | E | ✅ | unit | Dragging a tab out to a new window ❌ |
| T-09 | Tab from clipboard path | I | ✅ | manual | Ctrl+Shift+Alt+T (resolves the doc conflict, OBS-03) |
| T-10 | Drop files on tab | I | ✅ | manual | |
| T-11 | New-tab placement | O | ✅ | unit | |
| F-01 | File list columns | E | ✅ | build | Columns hide by width (removal order) |
| F-02 | Sort per folder | E | ✅ | unit | |
| F-03 | Folders on top | E | ✅ | unit | Inherited from ancestors |
| F-04 | Quick filter | E | ✅ | unit | |
| F-05 | Age colours | I | ✅ | unit | |
| F-06 | Inline metadata | I | 🟡 | | Dimensions in the details pane only; no page counts |
| F-07 | Long names (F7) | I | ✅ | manual | |
| F-08 | Hidden / temp files | E | ✅ | unit | |
| F-09 | Selection incl. middle-drag band | E | ✅ | manual | |
| F-10 | Folder toolbar | E | ✅ | build | |
| F-11 | Auto refresh + storm guard | E | ✅ | unit | |
| F-12 | Live sort after rename | I | ✅ | manual | |
| F-13 | Scroll mode | O | ❌ | | |
| F-14 | Folder sizes | O | ✅ | unit, self-test | Ctrl+Alt+S / context menu / Properties, or automatically (setting): total of everything inside in the Size column, sorting by size includes folders. Parallel, stays on one file system, hard links once, cancelled on leaving the folder |
| F-15 | Git status in the file list | O | ✅ | unit, self-test | Badges (modified / staged / untracked / conflict, folders by contents), ignored dimmed, branch with ahead/behind in the status bar. The repository's filters, hooks, fsmonitor and submodules are never run |
| V-01 | Built-in views | I | 🟡 | build | Adaptive view missing |
| V-02 | Per-folder view inheritance | I | ✅ | unit | |
| V-03 | Auto thumbs view | O | ❌ | | |
| V-04 | Quick view buttons | O | 🟡 | | View menu button instead |
| V-05 | Custom view editor | A | ❌ | | |
| P-01 | Properties + toolbar | E | ✅ | build | |
| P-02 | Image zoom/pan | E | ✅ | manual | GPS locate ❌ (needs EXIF) |
| P-03 | Text preview | E | ✅ | unit | |
| P-04 | PDF preview | I | ✅ | manual | pdf.js |
| P-05 | Video/audio | I | ✅ | manual | Depends on GStreamer codecs |
| P-06 | Preview window (Space) | I | ✅ | manual | External viewers ➖ |
| P-07 | Archive listing | O | 🟡 | unit | Zip-family only |
| P-08 | Email preview | O | ❌ | | |
| O-01 | Open | E | 🟡 | manual | Open-with chooser and run-with-args ❌ |
| O-02 | Clipboard copy/cut/paste | E | 🟡 | manual | Inside Gall ✅. Interop with other file managers is text paths only |
| O-03 | Copy/move to other browser | E | ✅ | unit, self-test | Conflicts: skip, replace (to the trash), keep both, merge folders with replace older / replace / skip / keep both for the files in both |
| O-04 | Rename popup, F2 cycling | E | ✅ | unit, self-test | Multi-file → batch rename ✅: template tokens, find & replace (regex), case, counter, edit as text; swaps allowed; one undo |
| O-05 | Duplicate | I | ✅ | unit | |
| O-06 | Trash / permanent delete | E | ✅ | unit | |
| O-07 | New folder variants | E | ✅ | unit | |
| O-08 | Templates | E | ✅ | unit | |
| O-09 | Notes .2do/.2dx | I | ✅ | unit | |
| O-10 | Clipboard → file | I | ✅ | unit (naming) | URL download ❌ |
| O-11 | Drag & drop | E | 🟡 | manual | Internal and drop-in ✅, live move/copy label ✅; drag-out to other apps ❌ |
| O-12 | Copy paths | I | ✅ | manual | |
| O-13 | Properties dialog | I | ✅ | manual | |
| O-14 | Colour tags | I | ✅ | unit | Stored in Gall DB, not in file metadata |
| O-15 | Undo | I | ✅ | unit | Create, move, rename, trash |
| O-16 | Task queue with progress | I | ✅ | unit | |
| O-17 | Paste as links | O | ✅ | unit | Symlinks |
| O-18 | Archives compress/extract | O | ❌ | | |
| O-19 | URL video download | O | ❌ | | |
| O-20 | Symlink | O | ✅ | unit | Hard links ❌ |
| O-21 | Context menus | E | ✅ | manual | Own menu; desktop service menus ❌ |
| S-01…S-04 | Sidebar | E | ✅ | build | Map network drive ➖ (use system mounts) |
| Q-01 | Search | I | ✅ | unit | |
| Q-02 | Tag search | O | ✅ | unit | |
| Q-03 | Indexed instant search | A | ❌ | | |
| A-01 | Themes | I | 🟡 | build | Dark/light/system + CSS themes. Named theme pack ❌ |
| A-02 | Accent colour | I | ✅ | manual | |
| A-03 | Font / size / padding | I | ✅ | manual | |
| A-04 | Thin scrollbars | O | ✅ | manual | |
| A-05 | Icon packs | O | ❌ | | |
| A-06 | Animations toggle | O | ✅ | manual | |
| C-01 | Searchable settings window | E | ✅ | manual | |
| C-02 | JSON settings + backup recovery | E | ✅ | unit | |
| C-03 | Portable mode | O | ✅ | | `portable` marker |
| C-04 | Shortcut editor | A | ✅ | unit | |
| C-05 | Start-up / new-tab paths, env expansion | I | ✅ | unit | |
| X-01 | Command line + single instance | I | ✅ | unit | `-nowindow`/`-openwin` ❌ |
| X-02 | Terminal here | I | ✅ | manual | Elevated ❌ |
| X-03 | Scripts menu | A | ❌ | | |
| X-04 | Batch tools | A | ❌ | | Next milestone |
| X-05…X-08 | Admin, default FM, Open/Save dialog integration, MTP | O | ➖/❌ | | |

**Essential features: 34 of 41 ✅, 7 🟡, 0 ❌.** The 🟡 items are listed above.
Nothing in this table claims parity with the original until the `manual` and `OBS:` checks are done.
