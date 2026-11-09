# Changelog

## Unreleased

## 0.2.0 — 2026-11-09

- Fixed: on Windows, cloning a repository and creating a worktree no longer fail with "could not open … .git/HEAD" (the destination was passed to Git as a `\\?\` path).
- Fixed: WezTerm is started as a new process, so "Terminal here" no longer asks an already running WezTerm (for example on another desktop) to open the window, which produced empty windows and needed a second keypress. A terminal command you saved earlier keeps its old text; edit it in Settings › Window or choose the terminal again with Ctrl+Enter.
- Fixed: Linux packages (deb, rpm, AppImage) now declare Gall as a handler for folders (`inode/directory`) and as a file manager, so it appears in the File manager list of Plasma's Default Applications and can be chosen for Meta+E.
- Improved: the commit context menu in the Git workspace now offers **Rebase current branch onto this commit…** (under Rewrite history) and **Merge this commit into current branch…** (under Integrate), as in Git Graph and GitLens. They have no key on a commit because lazygit has none there (rebase is `r` and merge `M` on a branch, `b` opens bisect options); commit-panel keys follow lazygit.
- Improved: Git workspace menus have one job each. Right-click, `Shift+F10`/Menu key or a row's **⋯** button opens the menu of that commit or reference; the toolbar **Actions** button holds repository-wide actions (fetch, pull, push, force push with lease, stash, refresh, search, terminal, continue/skip/abort). The separate "Panel actions…" button is gone, because **+** already creates items.
- Improved: the Git workspace footer shows the keys of the focused panel (like lazygit's options bar) and how to move around: `h`/`l` move between the sidebar, the commit list (with Working Changes as its first row) and the details, `[`/`]` switch sidebar tabs, and `Esc` returns to the commit list (also from Working Changes). An unsent commit message is kept when you leave Working Changes and come back. The panel with keyboard focus is outlined and named in the footer (Commits, Branches, Commit details, Working Changes), and the selection in the other panels is shown muted.
- Improved: in the Git workspace `y` on a commit opens lazygit's copy menu (hash, abbreviated hash, subject, message body, full message, author, date, commit URL for GitHub, GitLab, Bitbucket and Codeberg remotes, whole-commit diff, tags). `l` or Enter opens the commit details, `b` opens the tools on bisect options, `w` on worktree options and `x` the repository tools (as in lazygit, `b` and `w` are bisect and worktree options); `Esc` or `h` returns from the tools to the commit list.
- Fixed: Git workspace refreshes no longer freeze while a fetch, push or clone is running. Long transfers are stopped only after three to five minutes without output (instead of a fixed total time), and a failed or cancelled clone removes the folder it created so the same path can be used again.
- Fixed: Git workspace tools no longer refuse repositories that only use the standard Git LFS filters (switching, pulling, interactive rebase and worktrees work again). A stale saved rebase plan no longer blocks later rewrites or leaves leftover plan folders, and cancelled remote jobs are always unregistered.
- Added: a [Git workspace](README.md#git-graph-and-revision-switching) with commit details, changed-file patches, parent selection, historical contents, file history/blame, two-revision comparisons, search, reference filtering and incremental history loading. Reflog remains optional and off by default.
- Added: branch/tag/stash/remote management; explicit checkout, merge/rebase/cherry-pick/revert/reset actions; resumable operation controls and a conflict inspection/resolution panel. Network fetch, fast-forward pull and push show cancellable progress; force push uses an explicit commit lease.
- Added: [Working Changes](README.md#review-and-commit-selected-files) with staged/unstaged groups, file/all staging, supported text line/hunk selection, initial commits, amend, recoverable file discard and ignore/exclude rules.
- Added: [history editing](README.md#edit-commit-history), with direct message rewording, fixup/squash/drop and commit movement, plus an interactive linear-range plan supporting pick, reword, squash, fixup, drop, edit stops and drag/keyboard reordering. Recovery references retain rewritten history.
- Added: repository tools for worktrees, recorded submodules, manual bisect, patches, notes and initialization/cloning. See the [coverage matrix](docs/git-workspace-roadmap.md) for implemented boundaries and remaining workflows.
- Improved: the Git workspace has grouped context menus (Rewrite history, Integrate, Remote, More), right-click selects without opening details, closes on click-away and offers checkout of a branch at a commit; a three-way conflict editor (Ours/Theirs/Both/Base with a live Result and manual edit), a banner naming the commit being merged or replayed, interactive rebase listing commits newest first like the graph (including rebasing onto a diverged branch), Ctrl+Enter to commit, a leave confirmation on Esc/`q`, ahead/behind counts, a clear hint to force push with lease after a rejected push, the current branch pinned and marked in References, and clearer error messages. Recovery refs are hidden unless Reflog is enabled; the graph reloads on window focus and no longer draws lines above branch tips.
- Improved: contextual lazygit-inspired direct keys and `?` help inside the workspace; `g r` opens it from the file browser, `= g` chooses a file-comparison revision from the same graph, and `= h` compares with HEAD. `=` is now a prefix key, so the former single-key compare is `= =`. `+ +` focuses the other browser.
- Fixed: graph browsing does not perform checkout or update working-file timestamps. Explicit checkout prepares dirty changes through stash/commit/confirmed discard; detached commits and discarded work retain recovery references or stashes. Git status shows the commit subject and current or containing local branches.

- Improved: application updates have their own Settings section between About and Shortcuts, and the About report button sits at the top right.

- Fixed (Windows): opening a filesystem `.lnk` shortcut follows its target through Gall's normal folder/file flow instead of handing folder navigation to Explorer. Program launchers and shortcuts with arguments keep their Windows shell behavior; broken targets report an error.
- Fixed: PDF preview reports read/render failures instead of leaving a blank page, and cancels pending document loads when switching files.
- Fixed: letter shortcuts now ignore Caps Lock; only holding Shift selects shifted-letter commands, as clarified in keyboard help.
- Fixed: Quick Open filters deleted files and folders from cached search results and recent paths, and refreshes the open picker when watched folders change.
- Added: Windows Explorer, Total Commander and Vim keyboard profiles in welcome setup and Settings › Keyboard, with saved selection and profile-aware help, menus, command palette and shortcut defaults. Commander uses F2 refresh, F3 preview, F4 edit, F5 copy, F6 move, F7 folder and F8 trash; Explorer uses F4 address and F6 pane focus. Existing custom shortcuts and leader choices are preserved across profiles. See [keyboard profiles](README.md#keyboard-profiles).
- Added: Ctrl+Insert copy, Shift+Insert paste, shared Gall shortcuts for editing and long names, and a leader sort group with all sort directions in every profile.
- Fixed: refresh file lists after moving items to trash, including partial failures, so F8 does not leave stale rows when a filesystem watcher misses the native operation.

- Improved: keyboard help and the next-key picker use individual keycaps with distinct simultaneous (`+`), sequential (`→`) and alternative (`or`) notation, explicit Shift for uppercase Vim commands, a visual legend and a no-results state. `?` now opens help in both key modes without intercepting text-field typing.

- Improved: keyboard help is directly available with `Shift+F1` / `Ctrl+/` even with Vim keys off, lists standard navigation and selection, and uses mode-aware hints. See [keyboard usage](README.md#usage).
- Added: Commander-style aliases for trash, rename, name search, selection, pane focus and sorting, extended by the selectable profiles above; `Space b p` pins or unpins the current folder, alongside existing Vim `b` and plain `Ctrl+B`. Every keyboard action now has a Vim or leader sequence, including `Space v` to toggle selection and `Space o m` for the context menu.
- Fixed: tab middle-click closes on release without activating an inactive tab or starting autoscroll, including older macOS WebViews without `auxclick`; Alt+middle-click transfer and reopening closed tabs remain available.

- Fixed: the support QR code no longer shows the poster's colored frame or a drop shadow.

- Added: a redesigned, local-first [bug reporter](README.md#report-a-problem) with optional reproduction details, screenshot capture, arrows/boxes/opaque redaction, silent short video clips, attachment previews and safe local exports. A separate review step leads to one **Copy & open GitHub** action; nothing is uploaded automatically.
- Changed: copied GitHub reports contain only the description, essential environment (including installation type and window resolution) and latest error. Recent actions, UI context and logs are included by default through **Save report...**, can be excluded, and are never copied. The report explains public attachment visibility before the GitHub handoff; closing the dialog keeps the draft in memory for the current application session.

- Improved: GitHub bug-report forms are filled in automatically with the short report, with a clipboard backup. A live encoded-link counter shows remaining space and recommends attaching a saved ZIP when the report is too long. Selected screenshots have a separate Copy screenshot action for pasting after the report.
- Added: Save report creates one ZIP with the report and all current screenshots and videos; optional extra diagnostics remain saved-only.
- Improved: suggested report filenames include a timestamp, successful saves return to the report automatically, and compact reports omit the unused Webview placeholder.
- Fixed: Escape cancels report dialogs with Vim keys on or off; after a successful GitHub handoff, reopening the reporter starts a fresh draft without discarding attachments still available in the current window.

- Fixed: GitHub bug reports retain the description, environment and latest error when the report exceeds the browser link limit.
- Fixed: bug-report export uses a bounded in-app path form instead of the system save dialog and protects existing files from overwrite.

- Improved: Report a bug is a distinct, themed button with a compact activity icon and no trailing arrow in About and Settings → About.

- Added: local bug-report preparation in About, with recent diagnostic events, optional log review and screenshot selection, GitHub form handoff, and previous-session diagnostics after an irregular exit.

- Improved: the update panel in Settings → About uses themed controls, clearer status messages and a responsive layout, with technical errors available on demand.

- Added: stable-release update checks at startup and in Settings → About, with signed installation for supported bundles and manual upgrade guidance for portable or system-managed installations. Release builds require updater signing configuration.

- Fixed: automatic folder-size calculation no longer interrupts Calculate now when the folder listing refreshes; obsolete progress and results cannot overwrite a newer calculation. Watched changes invalidate cached totals and, with Calculate automatically enabled, refresh visible folders.
- Fixed (Linux): folder sizes no longer report misleading totals such as 128 TB for `/proc`. Kernel virtual filesystems are detected before scanning and show `—` with an explanation in the Size column and Properties; ordinary files on `tmpfs` are still counted.

## 0.1.3 — 2026-11-05
- Usage: [search scope and exclusions](README.md#new-in-013-search-scope-and-exclusions) and [WSL browsing](README.md#windows-browse-wsl-distributions).
- Added (Windows): browse installed WSL distributions from Drives, start stopped distributions, open a terminal there and copy Linux-format paths. File operations use the Linux trash where available; Stop terminates the distribution after confirmation.
- Added: a support card in About and Settings, with featured tips shown first.
- Fixed: batch rename no longer overwrites a file whose name collides with a temporary staging path; a failed undo stays below operations completed while it was running.
- Fixed: a cross-drive move that cannot finish removing its source preserves the destination copy in the undo journal and attempts to restore partially removed source entries.
- Fixed: a failed note save keeps the draft open for retry instead of discarding it.
- Fixed: extracting archive entries no longer aliases names that need filesystem sanitization, races with another extraction, or overwrites members named `.used` with cache bookkeeping.
- Fixed: find-in-files include patterns no longer bring back files hidden by `.gallignore`; hiding an item during an on-demand folder scan no longer lets that scan restore the hidden result.
- Fixed: Quick Open no longer offers Alt+I inside archives, lets a delayed coverage check override the chosen scope, or applies stale search results and errors after changing the query or closing the window.
- Fixed: Quick Open respects input-method composition and AltGr input, and no longer adds opened folders to the recent-file list.
- Fixed: Quick Open no longer offers the unusable "this folder" scope inside archives.
- Changed: Quick Open starts in the open folder and below; Tab searches everywhere (it was the other way round). With no match in the folder it says so and points to Tab.
- Fixed: a WSL folder listed in Settings › Search › Indexed folders was still read again by Quick Open instead of using the index.
- Fixed (WSL): copying, duplicating or moving a folder inside WSL was refused when a file somewhere inside it had a name ending with a dot or space, although Linux does that copy; the whole folder was also read beforehand. Only a permanent delete, a copy or move to Windows and archives still check every name.
- Fixed (WSL): undo of several trashed items stopped at the first whose place was taken again; the others now come back too.
- Fixed (WSL): undo restores the newest copy when the same path was trashed more than once within a second.
- Fixed: Alt+I in Quick Open passed Quick Open's syntax (`ext:pdf`, `'exact`, several words) to the name search, which then found nothing.
- Changed: Hide from search no longer re-indexes the whole computer for each item; the cache-folder check costs one attribute query per folder.
- Fixed: Quick Open found nothing in a WSL distribution, on a network share or in a folder outside the indexed ones. It now opens scoped to such a folder and starts reading it as soon as it opens (reused for two minutes), so results are there by the time you type. Find in files › Home inside WSL searches the Linux home instead of the Windows one.
- Added: per-project search exclusions. A `.gallignore` file (gitignore syntax, also outside git) hides files and folders from Quick Open, find in files and the name search (F3) in its folder and below, also when a search starts in a subfolder. Right-click › Hide from search adds the selected items to it (with Undo); Quick Open drops them at once.
- Added: items hidden from search are dimmed in italics; right-click › Show in search again takes the line out, or adds a `!` exception when a pattern hides the item. A hidden folder says "hidden from search" in its header (click to show it again). Hand edits to a `.gallignore` reach Quick Open without a manual re-index.
- Added: Alt+I searches ignored files too: a toggle in find in files and the name search (F3), and in Quick Open it searches the current folder by name including what the index leaves out.
- Added: folders tagged as caches (`CACHEDIR.TAG`, e.g. Cargo's `target`) are skipped by every search. New installs also skip `coverage`, `.next`, `.nuxt`, `.svelte-kit`, `.turbo`, `.parcel-cache`, `.gradle`, `.tox`, `.mypy_cache` and `.pytest_cache`.
- Changed: the name search (F3) skips the folders in Settings › Search › Never index or search (node_modules, target, dist, …) and, with *Skip files ignored by .gitignore*, what `.gitignore` leaves out, like Quick Open and find in files.

## 0.1.2 — 2026-11-04
- Changed: the compare view shows the folders of both files as ~/… inside the home folder (it showed the full path).
- Added: the .deb and .rpm install the AppStream metadata, so KDE Discover and GNOME Software show Gall's description and screenshots. Releases include a SHA256SUMS file.
- Fixed (Windows): with Explorer's default "hide extensions for known file types", Ctrl+Z of a delete or Replace said the items were no longer in the trash, and the sidebar's Trash listed and restored items without their extension (a.txt came back as a). Names are now read from the Recycle Bin itself; a folder known by its short 8.3 name (C:\Users\LONGNA~1\…) or in other letter case is matched too.
- Security: spreadsheet compare uses calamine 0.38, which fixes a crash and unbounded memory use on malformed .xls/.xlsb files (RUSTSEC-2026-0436).
- Fixed: Settings list fields (extensions, excluded folders, start folders) dropped the comma just typed, so no item could be added at the end. Number fields saved every key on the way ("1" on the way to "16" set a 1 px font) and now keep to their range; spin arrows still act at once. Shortcut fields no longer snap back to the default while being retyped and show a chord another action has. Values typed just before closing Settings are kept.
- Fixed: vim s marks items one by one (the first s unmarked the item, and j/k dropped all marks).
- Fixed: context-menu submenus (Compress, Folder sizes, …) open from the keyboard: Right / Enter / l, back with Left / h / Esc.
- Fixed: a click or leaving the window cancels a pending leader sequence; Ctrl+wheel no longer counts as a Ctrl tap; menus no longer show the leader key (\ or ;) for its old action.
- Fixed: with an input method (Japanese, Chinese, Korean) Enter that picks the characters no longer also confirms the rename or dialog.
- Fixed: keyboard focus in the welcome setup (Enter on Back, Skip or a choice no longer means Next), Properties and other pop-ups no longer pull the focus back while they update, and each dialog in a row of questions starts with the focus on itself.
- Fixed: Space in the colour tag popup removed the tag from every selected item.
- Fixed: Quick Open kept jumping back to the first result when the index finished refreshing, so Enter opened the wrong file.
- Fixed: Edit path opens a path pasted in quotes (Explorer's "Copy as path"), ~\ on Windows and file:// URIs.
- Fixed (Windows): Ctrl+Z after a delete always said the items were no longer in the trash (the Recycle Bin's deletion time was read in the wrong unit).
- Fixed (Windows): a folder whose subfolder is shown (in the other browser or a deeper column) could not be renamed, moved or deleted ("Access is denied"): Gall's own folder watchers held it. They now pause for the operation, and a browser inside a folder that went away leaves it.
- Fixed (Windows): Eject always failed with "not supported" (after moving the browsers off the drive). It now ejects USB drives, waits a moment while the drive is busy and says when files on it are still open.
- Fixed (Windows): AltGr characters typed in a folder (Polish ś, @ or { on many layouts) were taken as Ctrl+Alt shortcuts: AltGr+S calculated folder sizes, and the quick filter refused them. Slovenian \ and ` now reach their shortcuts.
- Fixed (Windows): with OneDrive Files On-Demand, find in files, thumbnails and text excerpts downloaded every cloud-only file. Find in files now skips them and says how many; thumbnails and excerpts are not loaded for them.
- Fixed: Ctrl+Z of a copy to a network drive failed on every try (no trash there) and blocked all older undos; it now offers to delete the copy for good.
- Fixed: a case-only rename with letters beyond ASCII (čebela → Čebela) was refused as "already exists" on Windows and macOS.
- Fixed: a copy of "Budget (2026).xlsx" was named "Budget (2027).xlsx"; it is now "Budget (2026) (2).xlsx". Copies of folders with a dot get the counter at the end ("my.project (2)", not "my (2).project").
- Fixed: folders browsed quickly could stay watched after leaving them, and a folder size calculation could keep running after leaving the folder.
- Fixed (Windows): file:// paths pasted from the clipboard became `C:/x` or a host name without `\\`; they are now native paths.
- Fixed: "Replace" into a folder reached through a symlink (or bind mount) to the source's own folder deleted the source. Such a copy is now a duplicate next to the original, and a move onto itself is skipped.
- Changed: "Replace" moves the existing file or folder to the trash instead of deleting it; one Ctrl+Z restores it (and removes the copy or moves the item back).
- Fixed: a move to another drive writes the copy to the disk (fsync) before it deletes the source. A copy that fails or is cancelled no longer leaves a half-copied folder behind, and a copy never truncates an existing file.
- Fixed: Ctrl+Z after a large delete restored only the items trashed in the first seconds; it now restores all of them (and the newest copy when a path was trashed twice).
- Changed: Ctrl+D duplicates all selected items as a background task with progress and cancel (it froze the window for large folders); the copies are selected as they appear.
- Changed: rename, new folder/file, undo, notes, path completion and conflict checks run off the main thread, so a slow or sleeping network drive no longer freezes the window.
- Fixed: saving in the compare view keeps symlinks (the file they point to is saved), writes a temporary file first so a failed save never damages the file, and works where there is no trash (without undo, and says so).
- Added: where the trash is not available (network shares, read-only trash), Delete offers to delete permanently and Replace offers to replace permanently, instead of only failing. On Windows, items on network drives are never sent to the Recycle Bin (Windows would delete them for good).
- Added: a link in a Markdown preview to a program or script (.exe, .bat, .sh, .desktop, …) asks before it runs.
- Changed: copies on Linux use `copy_file_range`: on btrfs and XFS a copy on the same drive is a reflink (a 2 GiB file copies in milliseconds and takes no extra space), elsewhere the kernel copies without passing the data through Gall.
- Added: merge folders. When a copied or moved folder already exists, "Merge" puts the contents together (the default); a second question decides for files that exist in both: Keep both, Replace, Replace older (only when the incoming file is newer) or Skip. One Ctrl+Z reverts the whole merge, also from the parent folder.
- Added: batch rename. F2 (or the context menu) with several items selected opens a window with a name template (tokens: name, counter with padding, date, time, folder, extension), find & replace (text or regular expression with $1 groups), case changes and "edit as text" (one name per line). The preview flags empty, invalid, duplicate and taken names; names may swap; one Ctrl+Z undoes the whole rename.
- Fixed: Windows device names (CON, NUL, COM1, …) are refused as file names.
- Added: Git status in the file list: M modified, S staged, U new, ! conflict next to names (folders show the state of what is inside), ignored items dimmed, and the branch with ahead/behind counts in the status bar. Updates when the folder changes and when the window gets the focus back. In repositories one did not create, none of their filter, hook, fsmonitor or submodule commands run. Setting: *Appearance › Items › Git status*.
- Changed: folders outside a Git repository no longer start git on every navigation.
- Added: folder sizes. Ctrl+Alt+S (leader f z, the context menu or *Folder sizes* in the background menu) counts everything inside the selected folders, or all folders here, and shows the total in the Size column with running totals while counting; sorting by size then sorts folders too. *Calculate automatically* does it for every folder you open. Properties (Ctrl+I) of a folder shows its total size. The count is parallel, stays on the folder's drive, counts hard links once and stops when you leave the folder.
- Fixed: copies (and moves to another drive) kept the default permissions for folders: a private 0700 folder became readable by others. Folders and files now keep the source's permissions, also while they are being copied, and folders keep their modified date. On Linux, extended attributes (download origin, desktop tags and ratings, ACLs) are copied too.
- Fixed: when moving cut items fails or is cancelled, what is still at its old place goes back on the clipboard.
- Fixed: colour tags and folder view settings follow every move and rename, also of the items inside a renamed folder, also on undo and through batch renames that swap names. Before, a move that was skipped or got a " (2)" name moved the tag to the wrong path.
- Fixed: a failed undo (a name taken in the meantime) no longer loses the step: fix the cause and press Ctrl+Z again. Undoing a move checks every name before it moves anything back.
- Fixed: "Replace" refuses a folder that contains what is being copied or moved (it would have removed the source with it). Folder compare refuses a folder together with its own subfolder.
- Fixed: files copied in other file managers as `file://` links with encoded characters (spaces, accents) paste as files.
- Changed: starting copies and moves, pasting, ejecting a drive, reading the clipboard and watching a folder no longer run on the main thread (no freeze on a slow drive or a busy clipboard owner). Note edits are saved atomically.
- Fixed: undoing a move that merged folders recreates the source folders it emptied.
- Fixed: undo could bring back the wrong item from the trash when the same name was trashed twice within a few seconds (for example a delete, then undoing a copy that had taken the name). Trash items are now matched to the millisecond.
- Fixed: after ejecting a USB drive (or when it is unmounted elsewhere or pulled out), browsers that showed it go back to where they were before it, else to the home folder, instead of staying on a folder that no longer exists. They leave before the eject, so an open folder no longer keeps the drive busy.
- Fixed: eject reported "No such file or directory" when the drive had in fact been unmounted (the first tool unmounted it but failed to power it off; the next tools then failed). A drive that is no longer mounted counts as ejected, and a real failure shows the first tool's message.
- Fixed: copying (or moving to another drive) a folder with a named pipe hung for good, and one with a socket failed. Pipes are created again, sockets and devices are left out (logged). Previews, compare, find in files and folder compare no longer try to read pipes, sockets or devices (a pipe could block them, and the find-in-files context even froze the window).
- Fixed: a browser whose folder is deleted, renamed or unmounted elsewhere goes to the nearest folder above that exists (it showed a "list failed … No such file" error).
- Fixed: folders that cannot be opened say why in words ("You have no permission to open “Locked”."). Opening a link to nothing says so (it did nothing).
- Changed: notifications wait while the pointer is over them (Undo no longer runs away); errors go away after 20 s and stay in the notification log.
- Fixed: the keyboard sheet (?) scrolls down; the columns that did not fit were only reachable sideways. Context menus at the bottom edge stayed a few pixels outside the window. The Quick Open preview path ended with a stray "/". Leading and double spaces in names are shown.
- Added: `scripts/ui-audit.sh`, an exploratory UI audit with hard data (long and foreign names, 5000 items, deep paths, locked and vanishing folders, small windows, both themes) that checks every state for overlapping or cut-off text and controls outside the window.

## 0.1.1 — 2026-11-03
- Added: archive explorer. Enter opens zip / 7z / tar.* as read-only folders; copy (y y / Ctrl+C), preview and open single entries, extracted on demand. Encrypted 7z and zip ask for the password (kept in memory only).
- Added: syntax colours in source previews (PowerShell grammar included); any file a grammar knows previews as source.
- Added: preview & details pane per browser; swap the two browsers' folders (Ctrl+U, leader w s, ⇄ in the title bar above the divider) while each keeps its panes; ‹ / › there hide one browser; double-click the divider to centre it.
- Added: Markdown rendered in the preview, vim scrolling and a key-hints bar in the preview window, a fallback for videos the webview cannot play (codec and still frame with FFmpeg).
- Added: vim / leader keys for archives (z x/f/o/z/c), duplicate, delete permanently, notes, reveal, next view; menus and Settings show the keys of the current mode.
- Added: Gruvbox theme (replaces Tokyo Night); theme syntax colours apply to previews too.
- Changed: menus, dialogs, Quick Open and other pop-ups are solid instead of translucent.
- Fixed: Esc after clicking the background of Settings and other pop-ups; splitters jumping on drag start; portrait images in image compare; Ctrl+P "dir/" lists folders; quick filter after Ctrl+P; WezTerm opening behind a console window on Windows.
- Fixed: drag & drop of files, folders, tabs and favourites under WebKitGTK (Linux).
- Added: UI errors are written to the application log.
- Added: drag hint next to the pointer: red "Move … to <folder>", green "Copy … to <folder>", updates live with Ctrl/Shift/Alt (O-11).

- Fixed: Ctrl/Shift during a drag no longer "stick". The real key state is read from GDK; a modifier applies only while held.
- Added: default drop action setting (Move / Copy / by drive / Ask), also in the folder context menu.
- Changed: preview & details pane is off by default; toggle with the pane button in the tab bar, Alt+P or Settings › Appearance (persisted). Fixed the pane not filling its width.
- Changed: holding Alt while dropping creates symbolic links (the menu is available as the "Ask" default). Deliberate deviation from the reference, which shows a menu on Alt.

- Fixed: date column was clipped; sort arrows were not aligned with their columns (widths now shared and font-relative).
- Fixed: drag label ignored modifier keys where the OS cannot report them (non-Linux fallback).
- Fixed: search result paths were mangled by right-to-left ellipsis; accent colour picker was a thin line.
- Added: `scripts/selftest.sh`, an end-to-end UI self-test (50 checks, screenshots) for debug builds.

- Added: GitHub CI (fmt, clippy, tests on Linux/Windows/macOS, cargo-deny licence policy), release workflow (installers for Linux, Windows, macOS), Dependabot, issue/PR templates, SECURITY and CODE_OF_CONDUCT, THIRD-PARTY-NOTICES generator, AppStream metadata, README screenshots script.
- Added: Miller columns start at the home folder when inside it (Finder-style, setting *Columns › Start columns at the home folder*).
- Changed: notifications button moved into the main toolbar (bell); it overlapped the status bar.
- Changed: root `package.json` for the Tauri CLI (`npm run dev`, `npm run build`).

## 0.1.0 — 2026-10-31
First MVP: layouts, Miller columns, tabs, file list and views, previews, file
operations with undo, clipboard, drag & drop, sidebar, path editor, search,
templates, notes, colour tags, settings and shortcuts. See `docs/04-compatibility-matrix.md`.
