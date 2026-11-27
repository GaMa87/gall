# Git workspace coverage and remaining work

Gall adapts documented GitLens, VS Code Git Graph and standalone Git Extensions
workflows to its file manager. Keyboard commands are contextual and inspired by
lazygit. This is a coverage specification, not a claim that every command, option,
setting or hosted service of those products is available.

## Sources and interpretation

**Observed:** the official documentation describes an inspection workspace, where
selecting a commit shows details/files and selecting another revision enables a
comparison. It also describes branch, stash, remote and history operations.

- [GitLens Commit Graph](https://help.gitkraken.com/gitlens/gl-commit-graph/):
  embedded details, comparisons, reference panels and worktrees.
- [GitLens core features](https://help.gitkraken.com/gitlens/gitlens-features/):
  blame, revision navigation, editor annotations and interactive history editing.
- [GitLens editions](https://help.gitkraken.com/gitlens/gitlens-community-vs-gitlens-pro/):
  free authorship/history features versus paid private-repository graph and service integrations.
- [VS Code Git Graph features](https://github.com/mhutchie/vscode-git-graph#features):
  graph/context actions, selected-revision comparisons, review state and graph controls.
- [Git Extensions browsing](https://git-extensions-documentation.readthedocs.io/en/main/browse_repository.html),
  [commits](https://git-extensions-documentation.readthedocs.io/en/main/commit.html),
  [history editing/conflicts](https://git-extensions-documentation.readthedocs.io/en/main/modify_history.html),
  [remotes](https://git-extensions-documentation.readthedocs.io/en/main/remote_feature.html)
  and [manual contents](https://git-extensions-documentation.readthedocs.io/en/main/).
- [lazygit v0.66.2 key reference](https://github.com/jesseduffield/lazygit/blob/v0.66.2/docs/keybindings/Keybindings_en.md):
  contextual commands; Gall adapts their meaning to graphical controls.

**Design decision:** Gall keeps one graph with lower inspection/composition panels
and a reference/stash/remote/conflict sidebar. It implements these behaviors
independently. Source availability below does not establish cross-platform runtime
verification; focused checks and screenshot review must accompany each change.

## Behavior matrix

“Implemented” means the flow exists in the UI and core. “Limited” names a usable
subset with concrete boundaries. “External” means use a terminal, another Git client
or a hosting/editor integration; there is no inactive placeholder claiming support.

| Flow | Status | Available behavior | Remaining options or limits |
| --- | --- | --- | --- |
| Graph and recovery history | Implemented | Colored lanes, refs, Working Changes above HEAD, read-only inspection, Reflog checkbox, reference filter, refresh, loaded-history search; load 200 more through 20,000 | Search is not a repository-wide Git query; no multi-ref/glob selection, adjustable columns/order, avatar service, ancestry highlighting or graph review state |
| Commit inspection | Implemented | Full message, author/committer/dates, parents, changed files and line counts; colored per-file patch; merge-parent selection; arbitrary two-revision comparison | No rich Markdown/issue links, annotated-tag detail sheet, signature verification or persistent reviewed-file markers |
| File archaeology | Limited | Historical text, rename-aware file history, line blame and navigation from a blamed line to its commit | History UI loads 200 entries; file previews are bounded, blame is text-only and limited to 1 MiB/10,000 displayed lines; no in-editor CodeLens, hover or heatmap |
| Comparison picker | Implemented | `= g` and file menu use the same graph; `= h` compares HEAD; comparison selection never switches files | Working-file lookup uses its current path; absence at that path reports an error |
| Working Changes | Implemented | Staged/unstaged groups, file/all stage/unstage, index-only and initial commits, amend, recoverable unstaged-file discard, untracked ignore/exclude | No commit-template/signoff/trailer/spellcheck settings or arbitrary commit option editor; hooks and signing disabled |
| Partial staging | Limited | Select individual changes or whole hunks and stage/unstage against a checked snapshot, preserving working contents | Ordinary tracked text only; whole-file fallback for added/deleted/renamed/binary/mode/no-final-newline or large patches; no selected-line discard |
| Checkout | Implemented | Explicit branch/detached checkout; dirty preparation with stash/commit/discard; retained detached commits/recovery stash | No keep-dirty/three-way checkout mode; filters/submodules and active operations restrict switching |
| Branches and tags | Limited | Local branch create/rename/delete-merged, upstream setting, local lightweight/annotated tag create/delete | No orphan-branch UI, forced unmerged deletion, arbitrary remote-ref deletion, signed tags or tag push; tag descriptions are entered at creation |
| Stashes | Limited | Save tracked/untracked changes, also selected files, keeping staged changes (`--keep-index`) or tracked files only; inspect; apply/pop/drop and branch-from-stash by reviewed object ID; conflicting pop retains stash; drop keeps recovery ref; every live stash is one labeled row of the all-reference history (square node, dashed edge to its base, `stash@{n}` badge) that opens the stash preview, without the index and untracked-files helper commits | No index-only or ignored-file stash options |
| Integrating commits | Limited | Merge, rebase onto selected commit, single cherry-pick/revert, soft/mixed/hard reset; recovery refs and stale-state checks | No merge-strategy/mainline/no-commit editor, batch cherry-pick UI or topology-preserving rebase; hard reset refuses any untracked/ignored files |
| Direct history editing | Limited | `r` message-only reword; `f` fixup, `s` squash, `d` drop; confirmed earlier/later movement | Older commits must be in current linear HEAD history. Direct combine/move-earlier cannot rewrite a root/merge predecessor; HEAD reword preserves index contents |
| Interactive history editing | Limited | `i` includes selected commit through HEAD; unchanged exclusive base; pick/reword/squash/fixup/drop/edit, message fields, drag/buttons/keyboard ordering; persistent edit/conflict stops | At most 1,000 linear commits; no root rewrite, merge topology, exec/break/label/reset directives or autosquash discovery; original HEAD retained |
| Conflicts and operation control | Limited | Inspect base/ours/theirs/result, choose whole-file side, external editor, mark resolved; continue/skip/abort; edit-stop amend/continue | No editable per-hunk three-way combiner; binary contents need side choice/external tools; merge cannot skip; unsupported externally started cases may need terminal |
| Remotes and network | Limited | Add/rename/remove remote, edit fetch URL; fetch/prune; fast-forward pull; current-branch push/set upstream; explicit expected-commit force-with-lease; progress/cancel | No arbitrary refspec/tag push, push-URL editor or login/key-management UI. Preconfigured credentials run noninteractively; cancelled pushes may already have completed |
| Worktrees | Limited | List/open; create in one keystroke for a branch, remote branch, tag or commit in a preset, editable location (nested ones hidden by a local exclude rule), undo while unchanged, `.worktreeinclude` copies; lock/unlock; remove, also with local changes after storing them; move, repair, prune; switcher, live state, compare, temporary worktree and one dialog to clean up | No drag of files or hunks between worktrees, command after creation, linked instead of copied folders or multiple Working Changes rows in one graph |
| Submodules | Limited | Inspect recorded modules; initialize/update to recorded commit with local-change guards | No add/remove/sync/configuration UI or recursive update; filter/submodule restrictions remain for general index/history operations |
| Bisect | Limited | Manual good/bad endpoints, good/bad/skip tests, pending-checkout retry, log, finish/reset to start | No automated test-command execution or bisect replay UI; externally started bisects may need their original client |
| Patches and notes | Limited | Email patch export of one or several commits without overwrite; checked patch application to index/worktree; add/remove default commit notes | Clean application required; no mailbox-series/am workflow or arbitrary notes namespaces/merge UI |
| Repository setup | Implemented | Initialize current folder, clone into a new destination; retain partial clone on failure/cancel | No account repository picker, credential wizard, bare/mirror/shallow options or automatic recursive submodules; filter-configured clone stays without checkout |
| Maintenance | External | Reflog and recovery refs support locating retained history | No fsck/dangling-object recovery UI, garbage collection/compression, mailmap editor, hooks/signing/LFS setup or Git-flow wizard |
| Hosting/cloud/AI | External | Ordinary Git transport works with configured remotes | No provider PR/issues/CI/review services, Launchpad, Cloud Patches, AI composition/review/rebase, agent session management or cloud workspaces |
| Editor and platform integration | External | Gall file viewer/editor and direct contextual keyboard controls | No VS Code extension APIs, CodeLens, live editor blame/heatmaps, Live Share, Visual Studio plugin or Explorer shell extension from Git Extensions |

## Keyboard and interaction contract

From the file browser, `g r` opens Git and `= g` chooses a working-file comparison
revision. Inside Git there is no `g` prefix: `?` lists commands for the focused
panel. In history `r` rewords, `f` fixes up, `s` squashes and `d` drops; the Reflog
checkbox is not bound to `r`. In Working Changes Space stages/unstages, `a` toggles
all, `c` focuses the message and `A` selects amend. In refs `r` rebases, while in
remotes it renames. Fetch is available through the relevant Actions menu/panel.

Interactive plans use `j`/`k` for selection and `J`/`K`, Ctrl+j/Ctrl+k or
Alt+Up/Alt+Down for oldest-to-newest ordering. In the history graph Ctrl+j/Ctrl+k
(or Alt+Down/Alt+Up) moves the selected commit earlier/later after confirmation,
matching the graph's newest-first presentation. Input fields retain normal typing.
With Vim disabled, buttons, native controls, Tab, arrows and drag remain available.

Working Changes diff (WN-35): in the diff, `]` and `[` jump to the next or previous hunk, `v` starts a line range
(moving extends it over the changed lines, `Esc` ends it), `x` toggles one line, and Space or Enter stages the chosen lines
(unstages in the Staged list); with nothing chosen Space acts on the hunk under the cursor. `d` discards the chosen lines
(or the hunk) from an unstaged file after a confirmation; the whole file is first saved as a hidden recovery reference
under `refs/gall/recovery`, and the same stale-diff token checks as staging apply. With the mouse: the checkbox of a line
toggles it, Shift+click extends from the last click, the hunk header button selects the hunk, and a file row can be dragged
between the Staged and Unstaged lists. The keystroke count of benchmark scenario 1 on this flow is in the baseline below; there is no
automated check of the line discard from the UI (the backend has a test; the keys and the dialog are untested).

## Validation and manual acceptance

Build the bundled UI before the Tauri executable. Run only the affected scenarios
for an ordinary change; these commands do not imply a full suite or platform check:

```sh
npm --prefix ui run build
cargo build -p gall --features tauri/custom-protocol
./scripts/selftest.sh --git-compare target/debug/gall
./scripts/selftest.sh --git-workspace target/debug/gall
./scripts/selftest.sh --git-baseline target/debug/gall
```

The runners use disposable repositories, isolated settings and local bare remotes,
and print their report/screenshot directory. They need a graphical KDE session and
Spectacle. Inspect the screenshots; an assertion or a saved image alone is not a
visual review. macOS behavior requires a separate check on that platform.

On Windows the same two scenarios run through `scripts\selftest-git.ps1` (default build `target\debug\gall.exe`):

```powershell
scripts\selftest-git.ps1 [-Scenario git-workspace,git-compare,git-baseline] [-Scales 1,1.25,1.5,2] [-OutputFolder <folder>] [-Wsl [-WslStrict]]
```

It builds the fixtures in a throw-away folder (`core.autocrlf false`, hooks `NUL`, an empty excludes file, an empty
global Git configuration), starts a portable profile whose file index covers only the fixture, and runs each scenario at
100, 125, 150 and 200 % (the app derives the zoom from the display's own scale through the `<mode>@<scale>` self-test mode).
It refuses to start on a locked or disconnected session (exit 2), keeps the machine awake while it runs, writes
`summary.json` (runs, assertions, screenshot paths) next to the PNGs, removes its folders afterwards and exits non-zero on any
native failure. `-Wsl` adds one `git-workspace` run with the repositories in WSL (`\\wsl.localhost\...`, fixture from
`scripts/selftest-git-wsl-fixture.sh`); it is skipped with a reason when WSL or Git inside it is missing, and its failures are
advisory unless `-WslStrict`.

Measured on Windows 11 (debug build, 2026-11-22): `git-workspace` 124 assertions and `git-compare` 27 assertions passed at all
four scales (228 screenshots). The WSL run passes 77 of 80 assertions; it fails the interactive rebase step
(`interactive reword and fixup execute through actual native helper`): the history is unchanged after "Start rebase…" on a
repository in WSL. This is an open product gap, not a test artefact, and has not been investigated further. Assertions that
assumed Linux (the Stash button was found by text and hit the "Stashes" tab; repository panels are hidden below 1000 CSS px,
as at 200 %; `/` path joins) are fixed in the scenarios. Only a sample of the PNGs was opened (compare result at 200 %, the
narrow workspace, the failure and sidebar shots), not every one.
Backend tests are grouped under `fs::gitgraph`, `fs::gitinspect`, `fs::gitworktree`,
`fs::gitrepository`, `fs::gitrewrite`, `fs::gitnetwork` and `fs::gitadvanced` in
`gall-core`. Some history tests launch the bundled Gall executable as the rebase
editor helper and are ignored by default: build it first and use their explicit
`--ignored` test selection with `GALL_REBASE_HELPER` pointing to that build. For example:

```sh
GALL_REBASE_HELPER="$(realpath target/debug/gall)" cargo test -p gall-core --lib \
  fs::gitrewrite::tests::interactive_rebase_with_bundled_helper -- --ignored
GALL_REBASE_HELPER="$(realpath target/debug/gall)" cargo test -p gall-core --lib \
  fs::gitrewrite::tests::quick_history_actions_with_bundled_helper -- --ignored
```

Manual acceptance should use a disposable repository:

1. Select commits, search, filter and toggle Reflog. HEAD, file contents and disk
   timestamps must stay unchanged; explicit checkout must name its target.
2. Inspect a root, merge, rename, deletion and empty commit. Compare two revisions;
   verify their direction, chosen parent, metadata, file counts and contents.
3. Partially stage two separate text edits, commit one, and verify the other remains
   unstaged. Check whole-file fallback, amend, stale snapshots and file discard recovery.
4. Create/rename a branch, create a tag, save/apply/pop a stash and inspect retained
   commits. Declining any confirmation must preserve state.
5. In linear history reword, fixup/squash/drop/move and use an interactive edit stop.
   Verify the selected commit is included, the base is unchanged, original HEAD is
   retained, and continue/abort works after reopening the workspace.
6. Cause a merge conflict and inspect all sides. Resolve, stage and continue; repeat
   with abort. During rebase verify the explained ours/theirs meanings.
7. Fetch, fast-forward pull and push against a local bare remote; test a diverged
   branch and stale force lease. Exercise cancellation without assuming remote rollback.
8. Exercise tools in disposable folders: dirty worktree removal refusal, protected
   submodule edits, bisect return to start, nonoverwriting export, patch checks and
   clone cancellation. Repeat keyboard flows with Vim enabled/disabled and ordinary
   message typing, plus wide/narrow layouts and both themes.

### Gall baseline: steps and window latencies (WN-30)

Measured by the `git-baseline` self-test (`scripts\selftest-git.ps1 -Scenario git-baseline`, on Linux
`./scripts/selftest.sh --git-baseline target/debug/gall`; both need Python 3), which carries out every scenario in the
running application on the repository `scripts/bench-lazygit.py` uses (`scripts/selftest_git_baseline_fixture.py`) and checks
the Git state it ends in. Keys include every typed character (in brackets); a chord is one key. Clicks count button presses of
the mouse; a submenu that opens under the pointer costs none. A key counts only where the keyboard focus really is: a step
that leaves nothing focused fails the scenario. Opening the workspace is not counted, as starting lazygit is not. Both routes were measured with Vim
keys on, at 100, 150 and 200 % with the same result (Windows 11, debug build, 2026-11-23). Every scenario reached the expected state.

| Scenario | Gall, keyboard | Gall, mouse | lazygit, Windows native | Git Graph / GitLens |
|---|---|---|---|---|
| 1 stage 2 lines and commit | 27 (15) | 6 clicks + 15 (15) | 24 (15) | not verifiable |
| 2 squash last 3, new message | 24 (16) | 9 clicks + 17 (16) | 24 (16) | not verifiable |
| 3 cherry-pick from another branch (6 keys before WN-33) | 3 (0) | 3 clicks | 8 (0) | not verifiable |
| 4 who changed line X, jump to its commit | 3 (0) + 1 clicks | 3 clicks | unsupported | not verifiable |
| 5 resolve one conflict, continue rebase | 9 (0) | 7 clicks | 9 (0) | not verifiable |
| 6 undo a bad amend (made in Gall; 9 keys and 2 clicks before WN-37) | 2 (0) | 3 clicks | 2 (0) | not verifiable |
| 7 find a named stash a week later, apply | 6 (0) | 4 clicks | 12 (7) | not verifiable |
| 8 undo a bad rewrite (made in Gall; 10 keys and 2 clicks before WN-37) | 2 (0) | 3 clicks | not measured | not verifiable |
| 9 cherry-pick three commits (WN-33) | 9 (0) | 5 clicks | not measured | not verifiable |
| 10 undo an amend made outside Gall (soft reset from the reflog) | 9 (0) + 2 clicks | 4 clicks | 2 (0) | not verifiable |

Scenario 2 counts Ctrl+A in the prefilled message as one key. Where a keyboard route has clicks, no key exists: choosing a
blame line (4) and switching the reflog on and returning to the list (10). Scenario 4 works only for a file that the
selected commit changed. The lazygit numbers are those of WN-61 below; the hosts differ in the order of branches, so one key of
difference there means nothing. Git Graph and GitLens run in VS Code, which is not installed on the measuring host.

| Window latency (median of three runs, at 100, 150 and 200 %) | Windows 11, debug build |
|---|---|
| open to first graph paint, 200 commits | 94 ms |
| open to first graph paint, 2000 commits | 98 ms |
| key to selection in a list of 2000 commits | 48 ms |
| key to the Git command palette on 2000 commits (WN-32, 150 %, one run) | 22 ms |
| key to open Working Changes | 63 ms |
| key to refreshed Working Changes (stage a file) | 143 ms |

A latency runs from the input to the frame after the one that shows the result, so it includes up to one extra frame
(17 ms). The application was warm. Opening a repository of 2000 commits loads the first 200, as for every size; the selection row was
measured with all 2000 listed. Linux and macOS were not measured.

The numbers are stored in `ui/src/gitBaseline.json`. The self-test fails when a scenario needs more clicks, or more keys beyond the
typed text, than stored there; latencies are reported next to their baseline and never fail, because they depend on the machine.
A task that shortens a scenario lowers its numbers in the same change; raising one needs a reason in the commit message.

### Lazygit yardstick, Windows native (WN-61)

Measured by `python scripts/bench-lazygit.py --scenarios all --runs 3` (driver and fixtures in
`scripts/bench_lazygit_core.py`, shared layout for WN-52). Every number below is **Windows native,
NTFS, never comparable with WSL or Linux rows**. lazygit 0.66.2, Git 2.56.0.windows.4, Python 3.12.10,
`pywinpty` 3.1.2 (ConPTY) with `pyte` as screen buffer, 200x50 cells, screen polled every 5 ms,
`core.autocrlf=false`, no user or system Git config, hooks off. The driver's own cost, measured on
`cmd /c echo` and an echo helper: 38 ms process start to first output, 11 ms key to screen; it is
included in all numbers (they include the terminal emulator, as the task requires).

Why this driver: ConPTY is what Windows Terminal uses, so lazygit runs unmodified; `pyte` gives the
rendered text so the scripts wait on what is on screen, not on timers. `tmux` does not exist on native
Windows, and a scripted terminal that exposes its screen would need a second tool for no gain.

| Start to first commit list and file list | first run | warm median of 3 | plain `git` (status, log, branch, stash) | lazygit on top |
|---|---|---|---|---|
| 200 commits | 1158 ms | 1174 ms | 303 ms | 871 ms |
| 2000 commits | 1111 ms | 1093 ms | 293 ms | 800 ms |
| large working tree (5000 files, 500 changed) | 1108 ms | 1073 ms | 322 ms | 751 ms |
| 300 branches | 3392 ms | 1151 ms | 307 ms | 844 ms |

"First run" is the first start after the fixture was built; the OS file cache cannot be dropped
without administrator rights, so it is not a true cold start. The plain `git` baseline is four
processes one after another, timed with Python (`hyperfine` is not installed), and it covers only part
of what lazygit loads, so "on top" overstates lazygit's own work.

Key to screen on the 2000-commit repository (median): moving the selection 93 ms, opening a commit's
files 95 ms, staging a file 399 ms (that last one is the time until `git diff --cached` shows the file,
and the driver polls with a Git process, so it is an upper bound, not a repaint time).

| Scenario | keys (typed text) | time | Git state verified |
|---|---|---|---|
| 1 stage 2 lines and commit | 24 (15) | 1.1 s | yes, 2 lines committed, 2 left unstaged |
| 2 squash last 3, new message | 24 (16) | 1.8 s | yes, 3 commits became 1, tree unchanged |
| 3 cherry-pick from another branch | 8 (0) | 0.8 s | yes |
| 4 who changed line X, jump to its commit | unsupported | | no `blame` action in the keybinding menus of the Files and Commits panels (filter checked with a control word) |
| 5 resolve one conflict, continue rebase | 9 (0) | 2.1 s | yes, main's side kept, no markers |
| 6 undo a bad amend | 2 (0) | 0.4 s | yes, HEAD restored (the amend itself is set up outside) |
| 7 find a named stash a week later, apply | 12 (7) | 0.5 s | yes, 4 stashes remain |

Keys include every typed character. Time is from the first key to the verified result and includes the
driver's waits for dialogs, so it is not a pure latency. All scenarios succeeded in 3 of 3 runs.
Scenario 6 only covers an amend; undoing a bad rebase was not measured. Moving between branches in
scenarios 3 and 5 uses the minimal number of `down` presses for this fixture's branch order.

### Lazygit yardstick, WSL (WN-52)

Measured by `python3 scripts/bench-lazygit.py --scenarios all --runs 3` run **inside** WSL (Ubuntu 26.04 on WSL2,
kernel 6.6.87.2-microsoft-standard), fixtures on the Linux filesystem (`/tmp`, never `/mnt/c`). Every number is **WSL**,
never comparable with the Windows native rows above or with Linux. lazygit 0.66.2, Git 2.53.0, tmux 3.6, hyperfine 2.0.0,
Python 3.14.4, 200x50 terminal driven through a detached tmux session, `capture-pane` polled every 5 ms (one tmux process per
poll). Driver cost measured on `echo READY` and a shell echo loop: 27 ms process start to output, 4 ms key to screen; included in
all numbers. Same fixtures, scenarios, assertions and report as the Windows rows (`scripts/bench_lazygit_core.py`).

| Start to first commit list and file list | first run | warm median of 10 | plain `git` (hyperfine, same four commands) | lazygit on top |
|---|---|---|---|---|
| 200 commits | 165 ms | 168 ms | 6 ms | 162 ms |
| 2000 commits | 162 ms | 178 ms | 8 ms | 171 ms |
| large working tree (5000 files, 500 changed) | 322 ms | 187 ms | 18 ms | 168 ms |
| 300 branches | 158 ms | 176 ms | 10 ms | 167 ms |

Key to screen on the 2000-commit repository (median of 12, 3, 3): moving the selection 15 ms, opening a commit's files 15 ms,
staging a file 8 ms (until `git diff --cached` shows it; a polling Git process is part of that, so it is an upper bound).

| Scenario | keys (typed text) | time | Git state verified |
|---|---|---|---|
| 1 stage 2 lines and commit | 24 (15) | 0.13 s | yes |
| 2 squash last 3, new message | 24 (16) | 0.44 s | yes |
| 3 cherry-pick from another branch | 8 (0) | 0.52 s | yes |
| 4 who changed line X, jump to its commit | unsupported | | no `blame` action in the keybinding menus of the Files and Commits panels |
| 5 resolve one conflict, continue rebase | 10 (0) | 0.61 s | yes |
| 6 undo a bad amend | 2 (0) | 0.03 s | yes (the amend is set up outside) |
| 7 find a named stash a week later, apply | 12 (7) | 0.36 s | yes |

All scenarios passed in 3 of 3 runs. Scenario 5 needed 10 keys here and 9 on Windows because the branch list order (one extra
`down` to reach the target branch) differs between hosts; the keystroke count of a scenario therefore carries its host.
The first run is not a cold start (the OS file cache was not dropped). Not measured: a native Linux host and macOS (no such host
was available; the script detects them and uses the same tmux driver) and undoing a bad rebase (scenario 6 covers an amend only).

## Completion boundaries

Every operation needs actual-application coverage, safe failure handling, stale-state
checks and independent review before it can be described as verified. Cross-platform
validation, interactive credentials, filters/LFS, repository hooks/signing and advanced
options in the matrix remain distinct work. Adding a menu item or a backend command
alone does not establish product parity.
