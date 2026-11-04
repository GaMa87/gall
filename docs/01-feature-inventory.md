# Feature inventory (Phase 1–2)

Reference product: OneCommander v3 manual and v4 beta 0.0.78.0 (Windows, WPF, .NET 10).
Source keys are explained in `00-clean-room-policy.md`.

**How this document is written.** Every entry is a short summary of *functionality* in our own
words. It contains no quoted text from the reference product's manual or UI, and no reproduction
of its internal file layouts. Reference feature names appear only where needed to identify the
feature being described; Gall uses its own, generic names in its UI.

Classification: **E** = Essential, **I** = Important, **O** = Optional, **A** = Advanced.
Platform column: how the Windows behaviour maps to Linux, or `N/A` when it cannot.

## 1. Application overview  [Observed]

- Purpose: a fast, keyboard- and mouse-friendly replacement for Windows Explorer
  that uses Miller-column navigation, one or two side-by-side "browsers", tabs,
  previews and caching. `WEB:/`, `M:welcome`
- Target users: power users who manage many files: photographers, engineers, developers. `WEB:/`
- Platforms: Windows 10/11 only, with high-DPI screens recommended. v4 requires .NET 10. `WEB:/`, `WEB:/beta`
- Free Home edition, plus a Pro licence that unlocks some settings. `WEB:/pro-features`.
  *Gall has no editions; every feature is available to everyone.*

## 2. Window structure and layouts

| ID | Feature | Class | Description / interaction | Source |
|----|---------|-------|---------------------------|--------|
| W-01 | Main window | E | Sidebar on the left, plus one or two **Browsers**. Window title shows app name and version. | `M:mainwindow` |
| W-02 | Standard layout | E | Browsers side by side. Each has a Navigation Pane (Miller columns) on top, with the Current Folder Pane and Details Pane below. | `M:browsersandlayouts` |
| W-03 | Columns layout | E | Browsers stacked vertically. The current folder is the rightmost Miller column, with the Details Pane to its right. | `M:browsersandlayouts` |
| W-04 | Classic layout | I | One browser: a single large file list with the preview on the right (Explorer-like). | `M:browsersandlayouts` |
| W-05 | Layout switch button | E | In the Main Toolbar. Changes only the layout; tabs, paths and settings are kept. | `M:mainwindow` |
| W-06 | Collapsible browser | I | Dragging the browser divider below a minimum shows a hint, and releasing collapses that browser (single-browser mode). An arrow or handle restores it to its exact previous state. A window too narrow for two browsers auto-closes one and reopens it when widened. | `M:browsersandlayouts`, `M:resizingpanesandbrowsers` |
| W-07 | Dividers with handles and arrows | I | Draggable dividers with a dotted drag handle and arrows. An arrow jumps the divider to its end stop; clicking again at the end stop restores the default position. Some panes auto-collapse below their minimum size. | `M:windows`, `M:resizingpanesandbrowsers` |
| W-08 | Main Toolbar | E | Bottom-left of the sidebar: Settings, About, Layout, Search, batch tools, New Window. | `M:mainwindow` |
| W-09 | Notification pane | I | Transient notifications at the bottom of the window. `~` (Shift+\`) or the up-arrow button shows recent history. Duration defaults to 6000 ms. | `M:mainwindow`, `RN:3.60` |
| W-10 | Multiple windows | I | New Window button. Each window has its own layout. Favourites are shared across windows. | `M:windows` |
| W-11 | Named windows / session restore | I | A named window remembers its tabs, layout and position. Unnamed windows do not persist. `-openwin <Name>` opens one. | `M:windows`, `M:browsersandlayouts`, `M:startingfromcommandline` |
| W-12 | Background running / tray | O | Closing the window keeps the process in the tray. Double-clicking the tray icon restores it; the tray menu has Show, Exit and Run as different user. Ctrl+Close fully exits. | `M:startingclosing` |
| W-13 | Global hotkeys | O | Win+Alt+O / Win+Alt+E summons the window. Optional Ctrl+\` docks it to the bottom half of the screen. Shift+F1 centres a lost window. | `M:shortcutkeys`, `M:window` |
| W-14 | Background images | O | `cover.jpg`, `folder.jpg`, `front.jpg`, `background.jpg` or `poster.jpg` in a folder is shown as the folder pane background (toggle in Settings > View). Only visible files are used. | `M:folderpane` |

## 3. Navigation

| ID | Feature | Class | Description / interaction | Source |
|----|---------|-------|---------------------------|--------|
| N-01 | Miller columns | E | Each ancestor of the current path is a column. Clicking a folder in any column opens it in the next column and replaces everything to the right. | `M:foldercolumnsandnavigationpane` |
| N-02 | Drives column | E | Leftmost column lists drives with a short label and usage. | `M:drivescolumn` |
| N-03 | Column compression and hover expansion | I | Ancestor columns shrink to fit. Optionally a column expands while hovered, with configurable delay and duration (e.g. 400 ms). Each column has an expand button in Columns layout. F8 toggles resizing. | `M:foldercolumnsandnavigationpane`, `M:columns` |
| N-04 | Breadcrumbs vs columns toggle | E | \` key or the Navigation Pane toggle button switches the Navigation Pane between simple breadcrumbs and Miller columns (Standard layout). Dragging the divider far enough also reveals it, with a hint. | `M:mainwindow` |
| N-05 | Keyboard navigation | E | Left/Right move between columns. Right or Enter enters the folder (Enter does not move focus). Backspace, Left or Alt+Up goes to the parent. Tab/Shift+Tab move between panes. Focused item shows a dotted "marching ants" box (configurable). | `M:foldercolumnsandnavigationpane` |
| N-06 | Click behaviour | E | Ancestor columns: single click. Current folder in Columns layout: double click (single click configurable). | `M:foldercolumnsandnavigationpane`, `M:other` |
| N-07 | Per-tab history | E | Alt+Left/Right or mouse buttons 4/5 go back and forward. Back/forward buttons sit above the file list. A tab created from a favourite has no history yet. | `M:foldercolumnsandnavigationpane` |
| N-08 | Up button | E | "↑" above the file list opens the parent. Clicking another column's title also navigates. | `M:foldercolumnsandnavigationpane` |
| N-09 | Path edit dropdown | E | Opened with `\`, Alt+D or Ctrl+L, or the Path Edit button. Type a path and press Enter to open it. Autocomplete (c:\w → c:\Windows, results limited for huge folders), filtering of favourites and recent tabs, a clipboard path suggestion, and ancestor list and history (`RN:3.85`, `RN:3.93`). Typing `explorer` opens the system file manager (`RN:3.95`). | `M:patheditdropdown` |
| N-10 | Path copy formats | I | Right-click the path box: `\`, `"\"` quoted, `\\` escaped, `/`, `"/"` quoted, `\\?\` long path, 8.3 short. Default format is configurable. Ctrl+Shift+Alt+C copies the current folder path. | `M:patheditdropdown`, `M:clipboard` |
| N-11 | Recent destinations | O | List of recent copy/move destinations, clearable. Count is set by `SavedDestinationsCount`, default 32. | `RN:3.80`, `RN:3.58` |

## 4. Tabs

| ID | Feature | Class | Description / interaction | Source |
|----|---------|-------|---------------------------|--------|
| T-01 | Tabs per browser | E | `+` button or Ctrl+T clones the current tab (or opens a configured path). Middle-click on a folder opens it in a new tab. | `M:tabs` |
| T-02 | Close tab | E | × appears on hover and becomes clickable after 0.5 s. Middle-click or Ctrl+W also closes. | `M:tabs` |
| T-03 | Reopen closed tab | E | Ctrl+Shift+T. | `M:tabs` |
| T-04 | Tab alias | I | Context menu > Change alias, or remove alias. Option to keep the alias when the folder changes (`RN:3.89`). Setting to show the full path instead of the folder name. | `M:tabs`, `M:other` |
| T-05 | Reorder and move tabs | I | Drag to reorder. Drag to the other browser, or Alt+middle-click, to move. Drop on the other browser's `+` to copy. Drag outside the window to make a new window. | `M:tabs` |
| T-06 | Close others / close stale | I | Context menu: close, close others, close stale (not accessed for 24 h). | `M:tabs` |
| T-07 | Many tabs | I | Tabs shrink, show the path in a tooltip, and the clicked tab expands slightly. The mouse wheel over tabs switches tab (`RN:3.91`). | `M:tabs` |
| T-08 | Tab state | E | Path, scroll position, selection and history. Inactive tabs release their listing (lazy reload). Tabs persist across sessions. | `M:tabs`, `WEB:/` |
| T-09 | New tab from clipboard path | I | Ctrl+Shift+T (listed in the shortcut table) or Ctrl+Shift+Alt+T (`M:clipboard`). *The docs conflict, because Ctrl+Shift+T is also reopen-closed-tab; see OBS-03.* | `M:shortcutkeys`, `M:clipboard` |
| T-10 | Drop files on tab | I | Copies or moves the dropped files into that tab's folder. | `M:tabs` |
| T-11 | New-tab placement | O | After the current tab, or at the end. | `M:other` |

## 5. Current Folder Pane (file list)

| ID | Feature | Class | Description / interaction | Source |
|----|---------|-------|---------------------------|--------|
| F-01 | File list | E | Name with icon, inline metadata, age, extension, date and time, attributes, size. Column headers have no titles, only sort arrows. | `M:folderpane` |
| F-02 | Sorting | E | Each column has ▲▼ arrows (bold = active). Sort button for the grid view. **Sort is remembered per folder.** Size sorts files only. | `M:folderpane`, `M:view` |
| F-03 | Folders on top | E | Toggle button, remembered per folder. Default set in Settings > View. "Sort folders by name always" option. | `M:folderpane`, `M:view` |
| F-04 | Quick filter | E | Type A–Z or 0–9. One character matches *starts with*; two or more match *contains*; a leading `.` matches the extension. Esc or × clears it. Space is reserved for preview. Applies to the current folder only. | `M:folderpane`, `M:basicfolderandfileoperations` |
| F-05 | Age column | I | Colour-coded time since last change (hot→cold, e.g. 1h10', 2d, 20d, 6m, 1y, 5y). Saturation and lightness are adjustable. | `M:folderpane`, `M:general` |
| F-06 | Inline metadata | I | Image dimensions, document page count, cloud status icon. Shown when there is room. | `M:folderpane` |
| F-07 | Long names (F7) | I | Toggles the other columns off so the full name shows. | `M:folderpane`, `M:shortcutkeys` |
| F-08 | Hidden/system/temp visibility | E | Ctrl+H toggles hidden files. Separate options for system and temporary files. | `M:view` |
| F-09 | Selection | E | Click; Ctrl+click; Shift+click; Ctrl+A; Ctrl+Space toggles; Ctrl+Up/Down moves focus without selecting. **Rubber-band selection with the middle mouse button, started from anywhere.** | `M:selectingandopeningfilesandfolde`, `M:shortcutkeys` |
| F-10 | Folder pane toolbar | E | Sort, File view, New (file/folder/note from template), Paste. | `M:folderpane` |
| F-11 | Auto refresh | E | Filesystem watcher. F5 or Ctrl+R forces a refresh. A watcher exclusion list, and a warning when some process floods the folder with change events. | `M:folderpane`, `M:advanced` |
| F-12 | Live sort after rename | I | The list re-sorts after a rename. | `RN:3.61` |
| F-13 | Scroll mode | O | Pixel or per-item scrolling. | `M:view` |
| F-14 | Folder sizes | O | Calculated and cached folder sizes (folder-size database in v4). | `WEB:/`, `WEB:/beta` |

## 6. File views

| ID | Feature | Class | Description | Source |
|----|---------|-------|-------------|--------|
| V-01 | Built-in views | I | Default, Name only, PowerShell (attributes, date, size, name order), Thumbnail grid (Ctrl+wheel resizes), Thumbs+Details (two-line, with metadata or a text excerpt), Adaptive (variable height). | `M:fileviews` |
| V-02 | Per-folder view with inheritance | I | The chosen view is stored per folder and inherited by subfolders unless overridden. Setting it on the drive root applies to the whole drive. Stored in a local database. | `M:fileviews` |
| V-03 | Auto Thumbs+Details | O | Image-heavy folders with no explicit view switch automatically. | `M:view` |
| V-04 | Quick view buttons | O | View buttons in the Name column header, when there is room. | `M:fileviews` |
| V-05 | Custom view editor | A | User-defined list columns: order, alignment, minimum width and a priority for which elements hide first when space runs out. Live preview on the current folder; designs can be saved, loaded and applied temporarily. | `M:fileviewdesignereditor` |

## 7. Details Pane and preview

| ID | Feature | Class | Description | Source |
|----|---------|-------|-------------|--------|
| P-01 | Properties sub-pane | E | Item count and total size of the folder; name, type, created, modified, age. Toolbar: Cut, Copy, Rename, Delete (not in Classic). | `M:detailspane`, screenshot `M:mainwindow` |
| P-02 | Image preview | E | Wheel zooms toward the cursor and dragging pans. Interpolation is configurable. GPS → Locate button opens a maps site. | `M:detailspane` |
| P-03 | Text preview | E | Scrollable and selectable, read-only. Maximum length is configurable. | `M:detailspane`, `RN:3.72` |
| P-04 | PDF preview | I | Left/Right change page (Up/Down stay for file navigation). Toggle between fit page and fit width. | `M:detailspane` |
| P-05 | Video/audio preview | I | Built-in player. Left/Right seek by seconds or by a fraction of the length (1/8 default). `[` and `]` step one frame. Wheel zooms. Autoplay option (preview window only). v4 uses FFmpeg. | `M:preview`, `WEB:/beta` |
| P-06 | Preview window (Space) | I | Larger window. Esc closes. Up/Down change file. Three view-mode buttons. Optional external viewer (QuickLook/Seer, Windows only). | `M:preview` |
| P-07 | Archive listing | O | Preview shows the first 200 entries. | `RN:3.86` |
| P-08 | Email (.msg/.eml) preview | O | Recipients and attachments. | `RN:3.107.1` |
| P-09 | Pane divider | I | Resizable split between preview and properties. | `M:detailspane` |
| P-10 | Folder preview | I | Folder thumbnail artwork. | screenshot `M:mainwindow` |

## 8. File operations

| ID | Feature | Class | Description | Source |
|----|---------|-------|-------------|--------|
| O-01 | Open | E | Enter or double click (single click optional). Ctrl+Enter is Open With (and Run as admin for .exe). Ctrl+Shift+Enter runs with arguments. A notification appears when opening 10 or more files. | `M:selectingandopeningfilesandfolde`, `RN:3.108` |
| O-02 | Clipboard copy/cut/paste | E | Ctrl+C, Ctrl+X, Ctrl+V. | `M:clipboard` |
| O-03 | Copy or move to the other browser | E | Alt+C / Alt+M. | `M:shortcutkeys` |
| O-04 | Rename | E | F2 opens a rename popup; F2 again toggles name/extension selection. Slow double-click renames (optional). A warning appears when the extension is removed. F2 on several items opens batch rename. | `M:shortcutkeys`, `RN:3.66`, `RN:3.90`, `RN:3.81` |
| O-05 | Duplicate | I | Ctrl+D for a single file, or Ctrl+C then Ctrl+V in the same folder. | `M:shortcutkeys` |
| O-06 | Delete | E | Del sends to the Recycle Bin (triggered on KeyDown). Shift+Del and Ctrl+Shift+Del delete permanently. | `M:shortcutkeys`, `RN:3.99` |
| O-07 | New folder | E | Ctrl+Shift+N opens a popup. Ctrl+Enter creates and opens it. Shift+Enter creates it and moves the selection inside. | `M:shortcutkeys` |
| O-08 | New file from template | E | Ctrl+N opens the template list (sorted by name), with a name field. Enter creates it; Ctrl+Enter creates and opens it. "Create file template" in the context menu copies any file into Templates. The "Templates folder" link opens the folder in Gall. | `M:newfiletemplates`, `RN:3.105` |
| O-09 | Notes / To-Do | I | Alt+N creates a `.2do` text note. Up to 4 lines show in the Name column. The checkbox marks it done and renames it to `.2dx`. Double click edits. | `M:folderpane` |
| O-10 | Clipboard to file | I | Ctrl+V with text creates `YYYY-MM-DD HH-mm-ss.txt`; with a bitmap it creates `.png`; with a URL it offers .txt, .url or download. | `M:clipboard`, `M:folderpane` |
| O-11 | Drag & drop | E | Drag always moves; Ctrl+drag copies; Alt shows a menu (Extract, Compress, symlink, hard link). A red label means move, green means copy. Optional Explorer-style rules. Drop targets: folder panes, columns, favourites, breadcrumb segments, tabs. Right-button drag is deliberately not supported. | `M:dragdrop`, `M:basicfolderandfileoperations`, `RN:3.74`, `RN:3.86` |
| O-12 | Copy path(s) | I | Ctrl+Shift+C copies the selected paths (or the current folder if nothing is selected). | `M:shortcutkeys`, `M:clipboard` |
| O-13 | Properties dialog | I | Ctrl+I or Alt+Enter. | `M:shortcutkeys` |
| O-14 | Colour tags | I | Ctrl+Alt+T opens the tag popup with numbered colours. The search window has a Tag tab. | `M:shortcutkeys`, `RN:3.57`, `M:searchingfilesfolders` |
| O-15 | Undo (v4) | I | Ctrl+Z undoes file operations through the operation history. | `WEB:/beta` |
| O-16 | Operation engine | I | Queued background copy/move with progress, opened from a button in the bottom-left corner. | `RN:*`, `WEB:/beta` |
| O-17 | Paste as shortcuts | O | Ctrl+Shift+V. | `RN:3.93` |
| O-18 | Archives | O | Compress built in. Extraction was removed in 3.103 and returned in v4. No browsing inside archives. | `M:archives`, `RN:3.103`, `WEB:/beta` |
| O-19 | URL video download | O | Paste a video URL to download it with yt-dlp. The command is editable. | `M:clipboard`, `M:downloadanywebsitevideoasafile` |
| O-20 | Symlink and hard link | O | Through the Alt-drag menu. | `RN:3.74` |
| O-21 | Shell context menu | E (Win) | Right-click shows the full Windows shell menu. Shift+right-click shows extended items. Custom entries include "Open in…" (new tab, other browser, new window), Create file template, Regenerate thumbnail and Restore from Recycle Bin. **Linux:** an own context menu plus user-defined actions. | `M:basicfolderandfileoperations`, `RN:3.78`, `WEB:/beta` |

## 9. Sidebar

| ID | Feature | Class | Description | Source |
|----|---------|-------|-------------|--------|
| S-01 | Collapsible sidebar | E | Alt+S. Splitter. Vertical window name when collapsed. | `M:sidebar`, `RN:3.84` |
| S-02 | Drives group | E | Letter, label, usage bar, used/total GB. `…` menu: Refresh everything, Map network drive, Browse phone. Eject for removable drives. Can be hidden. | `M:sidebar`, `M:drivescolumn`, `M:window` |
| S-03 | Favourites groups | E | A default group from the user folders (This PC, account, Documents, Desktop, Downloads, Music, Pictures, Videos, Recycle Bin…). New Group button. `+` adds the current folder; dropping a folder on the group title adds it. `…` menu: rename, move up/down, delete. Items can be reordered or moved between groups by dragging. Context menu: open in new tab here or in the other browser, remove, change alias. In dual layout, hovering shows [1][2] buttons to pick the browser. Clicking opens in the current tab; middle-click opens a new tab. Dropping files onto a favourite moves them (Ctrl copies). "Create default sidebar folders" restores the group. | `M:sidebar`, `M:favorites`, `M:window` |
| S-04 | History group | I | Recently opened folders across all windows, de-duplicated so a revisited folder moves to the top. Count is configurable. Can be hidden. | `M:sidebar` |

## 10. Search

| ID | Feature | Class | Description | Source |
|----|---------|-------|-------------|--------|
| Q-01 | Search window | I | F3, Ctrl+F or the toolbar button. Scope is the current folder recursively, or the drive root. Wildcards `*` and `?`. A Locate button jumps to the result. Enter opens several results (`RN:3.95.11`). No fuzzy search. | `M:searchingfilesfolders` |
| Q-02 | Tag search | O | Tag tab lists files carrying a colour tag. | `M:searchingfilesfolders` |
| Q-03 | Instant search (v4) | A | Indexed search host (SQLite) in v4. | `PKG:SearchHost/` (dependency names only) |

## 11. Appearance

| ID | Feature | Class | Description | Source |
|----|---------|-------|-------------|--------|
| A-01 | Themes | I | Several built-in dark and light themes, including translucent and minimal variants. An option follows the OS theme. A shortcut reloads the theme. Users can add their own theme files. | `M:themes`, `M:creatingthemes`, `PKG:Themes/` |
| A-02 | Accent colour | I | Used for hover, selection and borders. Some themes lock it. | `M:theme` |
| A-03 | Font, size, padding | I | Item font size and line padding. | `M:general`, `WEB:/pro-features` |
| A-04 | Thin scrollbars | O | | `M:theme` |
| A-05 | Icon packs | O | User-installable icon packs for files (by extension or exact name) and folders. Vector icons are preferred over bitmaps, and the most specific match wins. Toolbar icons can follow the theme's colours. | `PKG:ResourcesInstall/How to customize icons.txt` |
| A-06 | Animations | O | Global animation speed. Columns slide in. | `M:general`, `M:columns` |
| A-07 | Opacity / Mica / Acrylic / background image | O | v4. | `WEB:/beta` |

## 12. Settings and persistence

| ID | Feature | Class | Description | Source |
|----|---------|-------|-------------|--------|
| C-01 | Settings window | E | Tabs: General, Theme, View, Columns, Window, Preview, Other, Advanced (Experimental, Shortcuts editor). Searchable and non-modal in v4. | `M:settings`, `WEB:/beta` |
| C-02 | JSON settings | E | Settings in a JSON file, with a backup copy restored automatically on corruption and a previous copy kept when the dialog closes. Roaming settings are separate from machine-specific ones. | `M:settings` |
| C-03 | Portable mode | O | A marker file switches between portable and per-user storage. | `RN:3.61.2`, `PKG:How to customize icons.txt` |
| C-04 | Shortcut editor | A | Rebind shortcuts such as Alt+S, Alt+P and \`. | `RN:3.62`, `M:mainwindow` |
| C-05 | Start-up tabs / new-tab path | I | Absolute path or `%ENV%` expansion. | `M:other` |

## 13. Integration and automation

| ID | Feature | Class | Description | Source |
|----|---------|-------|-------------|--------|
| X-01 | Command line | I | `<path>`, `-o <path> [-newtab]`, `-p/-path/-<path>` (browser 1), `-p2` (browser 2), `-nowindow`, `-resetwindowposition`, `-openwin <Name>`. v4 adds several paths and setting changes. | `M:startingfromcommandline`, `WEB:/beta` |
| X-02 | Terminal here | I | F10 opens the terminal; Ctrl+F10 opens it elevated. The shell is chosen in Settings > Window. | `M:shortcutkeys`, `M:window` |
| X-03 | Scripts menu | A | Run user scripts (`.bat`, `.ps1`, `.py`…) on the selection. Interpreters are configurable, and scripts receive the relevant folders through environment variables. | `WEB:/`, `RN:3.79`, `RN:3.95`, `PKG:ResourcesInstall/Scripts` |
| X-04 | Batch tools | A | Batch rename (token-based rules and RegEx), image conversion and resize, scripts per file. Presets can be saved. Sort order sets the numbering order. | `M:fileautomator`, `RN:3.64.1`, `RN:3.94`, `PKG:ResourcesInstall/FileAutomatorPresets` |
| X-05 | Run as admin / different user | O | Ctrl+Shift+F12 restarts elevated. **Linux:** pkexec, low priority. | `M:startingclosing` |
| X-06 | Default file manager / Win+E | O | Experimental registry hook. **Linux:** `xdg-mime default`. | `M:advanced` |
| X-07 | Open/Save dialog integration | N/A | Injects the current path into Windows Open/Save dialogs. Windows-specific. | `RN:3.100`–`3.107` |
| X-08 | Phone (MTP), network drive mapping | O | **Linux:** gvfs/kio mounts appear as ordinary mounts. | `M:sidebar`, `M:phonestorage` |

## 14. Error handling (observed behaviours)

- A clipboard held by another process: retries, then names the blocking process in a notification. `M:clipboard`
- A watcher event storm: shows a warning about the flood of changes. `M:advanced`
- Corrupted settings: restored from the backup copy. `M:settings`
- A long-path workaround (`\\?\`). `M:patheditdropdown`, `RN:3.60.1`
- Path autocomplete in huge folders: results are capped. `RN:3.105`
- An elevated process blocks drag & drop (Windows UIPI). `M:dragdrop`

## 15. Feature dependencies

```
Settings(C) ─┬─> Theme(A) ─> all UI
             ├─> Views(V) ──> FileList(F) ──> Preview(P)
FS layer ────┼─> FileList(F) <─ Watcher(F-11)
             ├─> Navigation(N) ─> Tabs(T) ─> Windows(W-10/11)
             └─> FileOps(O) ─> Clipboard, DnD, Undo(O-15), Notifications(W-09)
Favourites/History(S) ─> Navigation(N)
Search(Q), BatchTools(X-04), Scripts(X-03) ─> FS layer + FileOps
```
