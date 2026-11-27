---
name: gall
description: >
  Search, grep and drive the Gall file manager from the command line without a display.
  Use when asked to find files by name, search file contents, see which files the user has
  selected or open in Gall, or open a folder, a file selection, a comparison or a Git view in
  Gall. Triggers: gall, file manager, "what do I have selected", "show this file in the file
  manager", find files, grep, compare two files or folders.
---

# Gall

Gall is the keyboard-first file manager. These commands need no window; add `--json` for
machine-readable output. Errors go to stderr and the exit code is 1.

## Without a window

- `gall search <query> [--dir <folder>] [--limit N] [--json]`: fuzzy file-name search under the
  folder (default: the current one). Prints one path per line; `--json` prints
  `[{"path","name","isDir","score"}]`.
- `gall grep <pattern> [--dir <folder>] [--regex] [--case|--ignore-case] [--word] [--glob <g>]... [--limit N] [--json]`:
  search file contents with `.gitignore` / `.gallignore` honoured. Plain output is `path:line:text`;
  `--json` prints `{"files":[{"path","matches":[{"line","text"}]}],"stats":{...}}`.
- `gall selection [--json]`: the folder, the selected items and the focused item of the Gall
  window the user used last. Fails (exit 1) when Gall is not running.

## Opens a view in Gall

- `gall <folder>`: open a folder. `gall --new-window <folder>`: in a new window.
- `gall select <item>...` (same as `--select`): open the folder with the item(s) selected.
- `gall diff <a> <b>` (same as `--compare`): compare two files or two folders.
- `gall git [repo]` (same as `--git`): open the Git workspace of a repository.

## Install this skill

`gall skill install` links it into `~/.claude/skills/gall` and `~/.agents/skills/gall`;
`gall skill print` writes it to stdout.
