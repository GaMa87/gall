# WSL support: status and manual test on Windows

[User instructions](../README.md#windows-browse-wsl-distributions) cover opening a
distribution, searching, file operations and stopping it. This page records implementation
details and manual checks, not a guarantee that every listed scenario has been verified.

Code: `crates/gall-core/src/fs/wsl.rs` (all WSL logic), plus small
hooks in `fs/drives.rs`, `fs/ops.rs`, `fs/listing.rs`, `util/terminal.rs`,
`src-tauri/src/commands.rs` and `ui/src/components/Sidebar.tsx`.

## What works

- **Distributions as drives.** Installed distributions (registry
  `HKCU\…\Lxss`, which ones run from `wsl.exe --list --running --quiet`) are handled
  like volumes on Linux. Running ones (and ones started in this session, even if they
  idle out again) appear at the end of the Drives group and the drives column. The
  others wait under "Not mounted": clicking one starts it ("starting…"), and it then
  moves up among the drives and opens. Roots are `\\wsl.localhost\<distro>\`
  (Windows 11) or `\\wsl$\<distro>\` (Windows 10). The first running distribution is
  used to check which host actually answers, and Gall switches to the other one if
  needed. Docker Desktop's internal
  distributions are hidden.
- **No cost without WSL.** Detection runs on a background thread, is cached for 20 s
  and never blocks the drive list. With no `Lxss` key, no `wsl.exe` is started at all.
  `wsl.exe` calls have a timeout (5 s for the list) and never open a console window.
- **Stopped distributions.** Opening a folder in one starts it first with
  `wsl.exe -d <distro> --exec /bin/sh -c "exit 0"` (30 s timeout). If that fails you get a
  clear message: "did not start within 30 seconds…", or "is not installed (anymore)"
  for an uninstalled one. The history check never starts a distribution.
- **Browse, preview, rename, copy, move** both ways (Windows ↔ WSL, WSL ↔ WSL) use the
  normal engine, so progress, cancel, conflict dialog and undo work as they do elsewhere.
- **The Linux trash instead of the Recycle Bin.** Windows has no Recycle Bin for
  `\\wsl` paths, so Del moves items inside WSL to the distribution's own trash
  (`~/.local/share/Trash`, freedesktop.org format, the one Linux file managers and
  Nautilus in WSLg use). The home folder comes from `/etc/passwd` and the user id
  `wsl.exe` logs in as (`DefaultUid` in the registry). Ctrl+Z restores, and so do undo of
  a copy into WSL and of "Replace" inside WSL. Refused with a reason, followed by the
  usual "Delete permanently… cannot be undone" question: items on Windows drives mounted
  in WSL (`/mnt/c`), the home folder, the trash itself, and distributions whose home
  folder is not found. Permanent deletes record no undo step. Gall's Trash window shows
  only the Windows Recycle Bin, not the Linux trash.
- **Copies inside WSL run in Linux.** Within one distribution `cp -a --reflink=auto`,
  between two a `tar` pipe (unpacked into a temporary folder next to the target and
  renamed, so "Keep both" names work). This is faster than going through Windows, and
  keeps permissions (executable scripts), owners and symbolic links. Conflict dialog,
  undo and cancel work as before. A single file shows progress by its growing size; a
  folder counts when it is done. Without GNU `cp` (Alpine), the copy goes through
  Windows as before.
- **Eject stops a distribution.** The eject button ("Stop Ubuntu (WSL)") and Eject in
  the menu run `wsl --terminate`. Browsers leave it first, and it goes back under
  "Not mounted".
- **Copy path → "Linux path (WSL)"** (Windows only; also selectable as the default copy
  format): `/home/u/x` for folders inside WSL, `/mnt/c/Users/…` for drive paths.
- **Linux names Windows cannot hold.** Before a copy or move from WSL into a Windows
  folder starts, the whole source tree is checked for names with `: * ? " < > | \` or
  control characters, reserved names (`CON`, `nul.txt`, `COM1`…), trailing dots or
  spaces, and names that differ only in case in one folder. If any are found, nothing is
  copied, and the error names up to 5 of them with their Linux folder plus a count of the
  rest. An individual source whose name ends in a dot or space is refused because
  Windows drops the dot or space from the path and could hit a different file.
  Copying, duplicating or moving a normally named folder within WSL does not reject
  such names inside it: Linux handles that operation. Permanent deletion, transfers
  to Windows and archive operations still inspect the tree. The listing shows these
  names through a `\\?\UNC\…` path; rename an individually inaccessible item in Linux.
- **Case-sensitive renames.** Inside WSL, `readme` → `README` is no longer treated as
  "same file, different case" when both exist, so the other file is never overwritten.
- **Paths.** UNC ↔ Linux conversion (`\\wsl.localhost\Ubuntu\home\u` ↔ `/home/u`, also
  `\\wsl$`, forward slashes and `\\?\UNC\…`). Breadcrumbs, Up and history treat
  `\\wsl.localhost\<distro>\` as a root. On Windows, typing `/etc` in the path bar opens
  it in the distribution of the open folder, or in the default distribution.
- **Terminal here** in a WSL folder runs `wsl.exe -d <distro> --cd <linux path>`, inside
  Windows Terminal when `wt` is installed.
- **Gall inside WSL (Linux build, WSLg).** WSL's internal mounts (`/usr/lib/wsl/*`,
  `/mnt/wslg/*`, `/mnt/wsl`) are hidden from Drives, and `/mnt/c` is shown as `C:`
  ("Windows").

## How it is tested

- Platform-independent unit tests in `fs/wsl.rs` (run on Linux): `wsl.exe` output in
  UTF-16LE with and without a BOM, in UTF-8, and translated (German); registry entries
  to distributions (default, state, hidden, invalid); host by build; UNC ↔ Linux
  conversion, including private-use characters; verbatim spelling; typed Linux paths;
  terminal command line (including `;` escaping for Windows Terminal); `/etc/passwd`,
  `.trashinfo` writing and parsing with `%`-encoding; Linux copy plans; Linux path
  format (`/mnt/c/…`); refusal of names that are not distributions; name problems,
  reserved names and case clashes; mount filtering inside WSL.
- **Operations against a fake WSL root.** `wsl::register_root` makes a temporary folder
  count as `\\wsl.localhost\<distro>\`. Tests then run real `TaskManager` tasks and the
  undo journal:
  - copy Windows → WSL, then undo: refused without the trash, then deleted for good
    after the user agrees
  - move WSL → Windows and WSL → WSL, each undone
  - without a known home folder: trash refused with nothing deleted, then a permanent
    delete that leaves no undo step; Replace refused, then Replace permanently
  - with a home folder: delete into the Linux trash (`.trashinfo` content, name
    clashes in the trash, folders), undo in the right order, undo of a copy and of
    Replace inside WSL, and what is refused (`/mnt/c`, home, the trash itself)
  - copies within and between distributions made by Linux (a counter proves it):
    executable bit, private file mode, symbolic link and a Linux-only name all arrive
    intact, including "Keep both" names and a move between distributions with undo
  - Keep both; rename and undo
  - copy/move with Linux-only names: nothing copied, nothing journaled
  - a name ending in a dot refused for delete, copy, duplicate and rename
- UI: `core.test.ts` checks ancestors, parent, basename, isInside and leaving a removed
  distribution on `\\wsl.localhost` roots.
- Automated checks do not replace the real-WSL checks below. See
  [Contributing](../CONTRIBUTING.md#before-opening-a-pr) for the current test commands;
  results depend on the platform and checkout being tested.

## Not verified without real Windows

- Actual `wsl.exe` behaviour: exit codes, how long a cold start takes, and that
  `--exec /bin/sh` works in every distribution.
- How the WSL 9P server shows Linux names that Windows forbids. The code assumes the
  same private-use mapping as DrvFs (`:` → U+F03A) and also accepts raw characters.
- Whether names ending in a dot or space come through `\\wsl.localhost` unchanged.
- How WSL symlinks look through 9P (real symlinks, or files). Copies keep a link as a
  link. Creating a symlink on Windows may need Developer Mode.
- Whether folder change notifications work on `\\wsl` paths. If they don't, F5 refreshes.
- The `\\wsl.localhost` → `\\wsl$` fallback on an older Windows 10.
- That renaming into `~/.local/share/Trash` through 9P works, and that Nautilus in WSLg
  shows (and can restore) what Gall trashed.
- That killing `wsl.exe` on Cancel also stops the `cp`/`tar` inside Linux right away.
  If it does not, the copy keeps running briefly after Gall has removed the partial
  target.

## Risks

- Every copy or move out of WSL to Windows walks the source tree once more before it
  starts. On huge trees (`node_modules`) over 9P that adds noticeable time. The walk
  can be cancelled.
- If 9P maps forbidden characters differently than assumed, those names are reported by
  the copy itself (the copy fails part-way and removes the partial item) instead of by
  the up-front check.
- `Drive` now derives `Default`, and WSL rows use `..Default::default()`, so new fields
  added on other branches still merge.
- A copy inside WSL that Linux cannot finish (permission denied on one file) stops
  and removes the partial copy. Through Windows it also failed, but item by item.
- Running state refreshes every 20 s, so a distribution started outside Gall moves from
  "Not mounted" to Drives up to 20 s later. Clicking it under "Not mounted" always works.

## Manual checklist (Windows 10/11 with WSL 2 and at least two distributions)

Setup: in Ubuntu, create the test files:
`mkdir -p ~/gt/docs && cd ~/gt && touch ok.txt 'a:b' CON 'dot.' docs/README docs/Readme && ln -s ok.txt link.txt && echo hi > ~/gt/hi.txt`

1. [ ] On a machine **without** WSL, or after `wsl --unregister` of every distribution:
   start Gall. No WSL rows, no error, no console window flashes, and start-up time is
   unchanged.
2. [ ] With WSL, after `wsl --shutdown`: start Gall. Within ~5 s every distribution is
   listed under Drives › "Not mounted" (collapsed), and none is among the drives.
   `docker-desktop*` is not listed. A distribution already running at start-up is
   among the drives.
3. [ ] Open "Not mounted" and click Ubuntu: it shows "starting…" and the "Starting
   “Ubuntu”…" note. Within a few seconds it moves up among the drives (and into the
   drives column) and its root opens. After idling for a minute it stays among the drives.
4. [ ] Tooltip and path show `\\wsl.localhost\Ubuntu\` on Windows 11 (`\\wsl$\Ubuntu\` on
   Windows 10 builds below 22000), and both work in the path bar.
5. [ ] Breadcrumbs / Miller columns: open `\\wsl.localhost\Ubuntu\home\<you>\gt\docs`.
   Each ancestor is clickable, Up stops at the distribution root, and Back/Forward and
   the History list work.
6. [ ] Path bar: with a WSL folder open, type `/etc` → opens `\\…\Ubuntu\etc`. With
   `C:\` open, `/etc` opens it in the default distribution.
7. [ ] Preview `hi.txt`, an image, and a large text file inside WSL.
8. [ ] Rename `hi.txt` → `Hi.txt` (case only), then undo. Rename
   `docs/README` → `Readme` (both exist): refused as "already exists", and neither file
   changes.
9. [ ] Copy a Windows folder (with sub-folders, >500 MB total) into `~/gt`. Progress
   shows, Cancel removes the partial copy, a second run completes, and Ctrl+Z asks
   "Undo without the trash?", then deletes for good.
10. [ ] Copy `~/gt/docs` to a Windows folder: refused before anything is copied, naming
    `Readme` (differs only in case). The Windows folder stays unchanged.
11. [ ] Copy `~/gt` to Windows: the message names `a:b`, `CON`, `dot.` and
    `docs/Readme` with their Linux folders, and says nothing was copied.
12. [ ] Copy only `ok.txt` and `link.txt` to Windows. Then Move `ok.txt` to Windows and
    undo (it goes back into WSL). Note what `link.txt` became.
13. [ ] Copy the same folder to the same Windows destination twice. The conflict
    dialog offers Keep both / Replace / Skip / Merge, and all of them work.
14. [ ] Replace a file **inside WSL** from Windows: "Replace permanently?" appears, and
    confirming replaces it.
15. [ ] Delete (Del) `~/gt/ok.txt`: "Moved to trash" with Undo. In a WSL shell,
    `ls ~/.local/share/Trash/files ~/.local/share/Trash/info` shows it and its
    `.trashinfo`. Ctrl+Z puts it back. Delete something under `/mnt/c` through
    `\\wsl.localhost\Ubuntu\mnt\c\…`: refused with "open it through its drive letter",
    then "Delete permanently" is offered.
16. [ ] Shift+Del a folder in WSL: permanent delete with progress, and Ctrl+Z does not
    offer to restore it.
17. [ ] WSL → WSL: copy and move `~/gt/docs` (with `README`/`Readme`) to Debian's
    `/tmp`. Both files arrive, and undo of the move brings them back. Copy `~/gt` within
    Ubuntu to `/tmp`: `ls -l /tmp/gt` shows `link.txt -> ok.txt`, and a script made
    with `chmod +x` is still executable. Copy a 2 GB file within Ubuntu: progress moves,
    and Cancel stops it and leaves no partial file.
18. [ ] Delete, rename or copy `dot.`: refused with "ends with a dot or space". No
    other file is touched. Check that `dot` (without the dot) was not created or
    deleted.
19. [ ] Permission errors: open `/root` → readable "access denied" message. Copy into
    `/usr` → access denied, and nothing is left half-copied.
20. [ ] Terminal here (context menu / shortcut) in `~/gt/docs`: a WSL shell opens in
    `/home/<you>/gt/docs` (in Windows Terminal if installed). Try a folder with a space
    in its name.
21. [ ] Uninstall a distribution (`wsl --unregister Debian`) while a browser shows it.
    Within ~20 s the row disappears and the browser goes back to a folder outside it.
    Opening an old `\\wsl.localhost\Debian\…` from History says "not installed".
22. [ ] Drag a file from a WSL folder to Explorer and from Explorer into a WSL folder.
23. [ ] Watch `~/gt` in Gall and `touch ~/gt/new` in the WSL shell: the file appears
    automatically, or after F5 (note which).
24. [ ] Eject (or the stop button) on a running distribution: a browser in it moves
    out, "Stopped “Ubuntu” (WSL)" appears, the row goes back under "Not mounted", and
    `wsl -l -v` shows it Stopped. Copy path → "Linux path (WSL)" on `~/gt` gives
    `/home/<you>/gt`, and on `C:\Users` gives `/mnt/c/Users`.
25. [ ] Linux build inside WSLg (`gall` started in Ubuntu): Drives shows `/`, `~` and
    `C:` (`/mnt/c`), and no `/usr/lib/wsl/...` or `/mnt/wslg/...` rows. Copy a file to
    `/mnt/c/Users/<you>`, then delete it (trash or permanent fallback).
26. [ ] Search in `~/gt` (Windows build): Ctrl+P opens scoped to `gt`, says "Reading gt…"
    and then "N items in gt"; typing finds `ok.txt`. Ctrl+F finds a word
    in it, F3 finds `*.txt`, and Ctrl+F › Home searches `\\wsl.localhost\Ubuntu\home\<you>`.
    Right-click › Hide from search on a subfolder: it is dimmed and gone from all three.
