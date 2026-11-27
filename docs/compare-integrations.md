# Using Gall as the diff and merge tool of other programs

Gall can compare files for another program (Git's `difftool`, Subversion's `--diff-cmd`, Mercurial's `extdiff`)
and resolve a merge in its standalone merge window (`mergetool`, `merge-tool-cmd`, `merge-tools`).

## Status

| Part | State |
| --- | --- |
| `gall diff A B` and `gall --compare A B`: open the compare window | works |
| `--` before names that start with dashes (`gall diff -- --odd -x`) | works |
| `gall diff --wait` and `gall merge` | implemented with a separate window for each waiting caller |
| Linux launcher action | available in launchers that show desktop actions |
| Windows Explorer action | opt-in through the per-user NSIS installer |
| macOS Finder Quick Action | install manually using the steps below |

| Platform | Verified round trips |
| --- | --- |
| Linux (Hyprland x86-64) | Git 2.56.0 difftool and mergetool, Subversion 1.14.5 diff and merge, Mercurial 7.2.4 extdiff and merge. The probes used disposable repositories with spaces and dashed basenames and handed off to a running Gall. Standalone checks covered a cold launch, dirty-merge cancellation, caller interruption, an output error, and preservation of output bytes and permissions. |
| Windows | Native CLI, installer and Explorer checks pending. |
| macOS (27.2 arm64, bundled `Gall.app`) | Bundled `Gall.app/Contents/MacOS/gall`: usage error (exit 2), cold `gall diff --wait` with spaces and leading-dash filenames + caller interruption (exit 1), already-running `Gall.app` handoff for Git 2.51.1 `difftool` and `mergetool` on a disposable repo with spaces and `sub/-dash file.txt` (cancellation exits non-zero and preserves conflict bytes and `0o755` permissions), and the Finder Quick Action shell script (1-arg exit 2, 2-arg exit 0 handoff). `svn` and `hg` were not installed on the test macOS host. |

## The command line

```text
gall diff [--wait] LEFT RIGHT                  compare two files or folders
gall merge [--wait] BASE LOCAL REMOTE OUTPUT   three-way merge (BASE may be an empty argument: no common ancestor)
gall diff -- LEFT RIGHT                        a name that starts with dashes is taken literally after `--`
gall compare-files [-- FILE]                   choose two files, or choose a second file for FILE
```

* Relative paths belong to the folder the command is run in.
* A name that starts with `--` is a flag unless it comes after a lone `--`; one dash is a path, as before.
* `--wait` makes the caller return only when the user has finished. `gall merge` always waits.
* Closing a loaded comparison finishes it. A merge finishes when you confirm the resolved text after making every conflict choice.
* A waiting caller returns **0** when finished, **1** if withdrawn or if the merge is closed without confirming, and
  **2** for a usage, launch, read or write error.
* Confirming a merge atomically replaces `OUTPUT` and preserves its file permissions. Before confirmation its contents stay as they were.

A waiting caller and a window exchange one small JSON file (`{"status":"accepted"}`, `{"status":"cancelled"}` or
`{"status":"error","message":"…"}`), written next to a private temporary name and renamed into place. A caller that is
interrupted creates `<result>.cancel`, which closes the window as cancelled.
If a temporary-filesystem error prevents Gall from returning the result after saving, restore access to the
temporary directory and close the window again to send the existing result.

## Git

```sh
git config --global difftool.gall.cmd 'gall diff --wait -- "$LOCAL" "$REMOTE"'
git config --global diff.tool gall
git config --global difftool.prompt false

git config --global mergetool.gall.cmd 'gall merge -- "$BASE" "$LOCAL" "$REMOTE" "$MERGED"'
git config --global mergetool.gall.trustExitCode true
git config --global merge.tool gall
```

Git runs the command through the shell with the variables quoted as written, so spaces in paths are safe; the `--`
protects names that start with a dash. `trustExitCode` makes Git take exit code 1 as "not merged" and leave the file
in conflict. For a file added on both sides Git's `$BASE` names a file that does not exist: Gall treats a missing base
as none.

Git 2.56.0's `git mergetool` has a separate limitation for repository-root paths starting with a dash. Its shell
script passes these paths to `dirname` and `git checkout-index` without an option delimiter, so it can give Gall
empty merge inputs. A dash at the start of a basename inside a subdirectory works.

## Subversion

Subversion calls the diff command as `CMD -u -L LABEL1 -L LABEL2 FILE1 FILE2`. Use a small wrapper that keeps the
last two arguments:

```sh
#!/bin/sh
# ~/bin/svn-gall-diff
[ "$#" -ge 2 ] || exit 2
left= right=
for operand do
    left=$right
    right=$operand
done
exec gall diff --wait -- "$left" "$right"
```

Then `svn diff --diff-cmd ~/bin/svn-gall-diff` (or `diff-cmd` in `~/.subversion/config`). For conflicts Subversion's
`--merge-tool-cmd` is called with `BASE THEIRS MINE MERGED WCPATH`:

```sh
#!/bin/sh
# ~/bin/svn-gall-merge  (BASE THEIRS MINE MERGED WCPATH)
exec gall merge -- "$1" "$3" "$2" "$4"
```

Set `merge-tool-cmd` under `[helpers]` in `~/.subversion/config` to the wrapper's absolute path.
Use `svn update --accept launch` to invoke it for incoming conflicts. With redirected input, also pass
`--force-interactive` to Subversion.

## Mercurial

```ini
[extensions]
extdiff =

[extdiff]
cmd.gall = gall
opts.gall = diff --wait --

[merge-tools]
gall.executable = gall
gall.args = merge -- $base $local $other $output
gall.premerge = False
gall.priority = 1
```

`hg gall` then runs `gall diff --wait -- DIR1 DIR2`. Mercurial uses exit code 1 for `hg gall` when it found differences.

## Shell actions

On Linux, select **Compare Files…** from Gall's desktop actions in a supported launcher. Gall asks for both files.

On Windows, run the NSIS installer as your user with `/GALLCOMPARE=1`. In File Explorer, right-click one file and
choose **Compare with Gall…**. On Windows 11, look under **Show more options**. Gall asks for the second file.
To remove the verb, run the installer with `/GALLCOMPARE=0` and choose reinstall if prompted. Uninstalling the
installation that owns the verb removes it as well.

On macOS, create an Automator **Quick Action** receiving files in Finder. Add **Run Shell Script** with input passed
as arguments and use the script below. Adjust the application path for a custom Gall installation. Save the action
as **Compare in Gall** and run it from the Quick Actions menu of two selected files.

```sh
[ "$#" -eq 2 ] || exit 2
exec /Applications/Gall.app/Contents/MacOS/gall diff -- "$1" "$2"
```

## Round-trip checks

Round trip each of the three tools on disposable repositories with: a path with spaces, a path that starts with a
dash, a merge that is cancelled (the output file is byte for byte as it was and the caller exits 1), a merge that is
accepted (exit 0, the output is the resolved text), a Gall that is already running (the caller still waits) and a
caller that is interrupted. Report what was run per operating system.
