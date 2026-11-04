# UI analysis (Phase 3)

Source: annotated layout screenshots and text in the manual (`M:mainwindow`,
`M:browsersandlayouts`, `M:folderpane`, `M:detailspane`). No screenshots are copied here.

## Structure (observed)
- **Sidebar (left, resizable, collapsible).** Rounded "group cards": Drives (letter, label, usage bar, used/total), favourite groups (title with `…` and `+`), History. A *New Group* button sits below, and the main toolbar is the icon row at the bottom.
- **Browser.** Tab strip on top with `+` at the end. In Standard layout the Navigation Pane (Miller columns, drives column first) sits above, with a divider, then the Folder Pane. In Columns layout the folder pane is the last column.
- **Folder pane header.** Folder name (large) and toolbar icons (sort, view, new, paste). Below are ↑ ← → and the folders-on-top toggle. Column headers are pairs of ▲▼ arrows with no titles (deliberate, `M:folderpane` FAQ).
- **Details pane (right of the folder pane).** Large preview on top, then a properties table (item count/size header; Name, Type, Created, Modified, Age), with a resizable divider.
- **Bottom right.** "NOTIFICATIONS ˄" button; transient toasts.
- **Visual language.** Flat, light-grey or dark surfaces, thin borders, an accent colour on the active tab and column borders, a dotted outline on the keyboard-focused item, and compact fonts tuned for high-DPI screens.

## Gall's interpretation (independent design)
- Two built-in palettes (dark/light) defined as CSS variables, plus an accent colour. Users can override the variables with a CSS file.
- Original line icons. Rounded group cards and a soft shadow on modals. The active browser is marked by an accent line.
- Keyboard focus outline ("marching ants") as a dashed border, optional.
- Differences by design: a searchable settings window (planned in the original), an undo button in the toolbar, and a `file://` copy format.
