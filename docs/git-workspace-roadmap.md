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
| Stashes | Limited | Save tracked/untracked changes; inspect; apply/pop/drop by reviewed object ID; conflicting pop retains stash; drop keeps recovery ref | No stash-branch action or selected/index-only/keep-index/ignored-file stash options |
| Integrating commits | Limited | Merge, rebase onto selected commit, single cherry-pick/revert, soft/mixed/hard reset; recovery refs and stale-state checks | No merge-strategy/mainline/no-commit editor, batch cherry-pick UI or topology-preserving rebase; hard reset refuses any untracked/ignored files |
| Direct history editing | Limited | `r` message-only reword; `f` fixup, `s` squash, `d` drop; confirmed earlier/later movement | Older commits must be in current linear HEAD history. Direct combine/move-earlier cannot rewrite a root/merge predecessor; HEAD reword preserves index contents |
| Interactive history editing | Limited | `i` includes selected commit through HEAD; unchanged exclusive base; pick/reword/squash/fixup/drop/edit, message fields, drag/buttons/keyboard ordering; persistent edit/conflict stops | At most 1,000 linear commits; no root rewrite, merge topology, exec/break/label/reset directives or autosquash discovery; original HEAD retained |
| Conflicts and operation control | Limited | Inspect base/ours/theirs/result, choose whole-file side, external editor, mark resolved; continue/skip/abort; edit-stop amend/continue | No editable per-hunk three-way combiner; binary contents need side choice/external tools; merge cannot skip; unsupported externally started cases may need terminal |
| Remotes and network | Limited | Add/rename/remove remote, edit fetch URL; fetch/prune; fast-forward pull; current-branch push/set upstream; explicit expected-commit force-with-lease; progress/cancel | No arbitrary refspec/tag push, push-URL editor or login/key-management UI. Preconfigured credentials run noninteractively; cancelled pushes may already have completed |
| Worktrees | Limited | List/open/create detached or on a new branch; lock/unlock; remove clean unlocked noncurrent worktree | No move/repair/prune/force-remove UI, existing-branch creation choice or multiple Working Changes rows in one graph |
| Submodules | Limited | Inspect recorded modules; initialize/update to recorded commit with local-change guards | No add/remove/sync/configuration UI or recursive update; filter/submodule restrictions remain for general index/history operations |
| Bisect | Limited | Manual good/bad endpoints, good/bad/skip tests, pending-checkout retry, log, finish/reset to start | No automated test-command execution or bisect replay UI; externally started bisects may need their original client |
| Patches and notes | Limited | One-commit email patch export without overwrite; checked patch application to index/worktree; add/remove default commit notes | Clean application required; no mailbox-series/am workflow or arbitrary notes namespaces/merge UI |
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

## Validation and manual acceptance

Build the bundled UI before the Tauri executable. Run only the affected scenarios
for an ordinary change; these commands do not imply a full suite or platform check:

```sh
npm --prefix ui run build
cargo build -p gall --features tauri/custom-protocol
./scripts/selftest.sh --git-compare target/debug/gall
./scripts/selftest.sh --git-workspace target/debug/gall
```

The runners use disposable repositories, isolated settings and local bare remotes,
and print their report/screenshot directory. They need a graphical KDE session and
Spectacle. Inspect the screenshots; an assertion or a saved image alone is not a
visual review. Windows/macOS behavior requires separate checks on those platforms.
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

## Completion boundaries

Every operation needs actual-application coverage, safe failure handling, stale-state
checks and independent review before it can be described as verified. Cross-platform
validation, interactive credentials, filters/LFS, repository hooks/signing and advanced
options in the matrix remain distinct work. Adding a menu item or a backend command
alone does not establish product parity.
