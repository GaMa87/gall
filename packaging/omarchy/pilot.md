# A week with Gall on Omarchy

Use an Omarchy test account or VM for the first trial. Install Gall from the
Omarchy package channel once the recipe is available there. Until then, a
locally built, verified Arch package can be installed in the test account's
system using `pacman -U`.

In a terminal, record the previous folder handler with
`previous=$(xdg-mime query default inode/directory)`. Keep this terminal open.
Use Omarchy's file-manager selector if your release offers it. Otherwise, to
try Gall for standard folder links, run `xdg-mime default gall.desktop inode/directory`.
Try `xdg-open "$HOME"`. Restore the previous choice in the same selector or
run `xdg-mime default "$previous" inode/directory` in that terminal. If there was no previous handler, use the selector to restore Nautilus
or restore the saved user MIME configuration in the disposable account.
The test package leaves the shared D-Bus activation file to the desktop.
Some launchers and reveal commands may still open Nautilus. On newer Gall
builds, a running Gall can also handle reveal requests while Nautilus is selected.
Record either mismatch and keep the system-default rollout on hold.

## Three things to try

1. From a project directory, run `gall "$PWD"`. Use Quick Open (`Ctrl+P`) to
   find a document in the project, then select it and press Space to preview it.
2. Select two disposable directories in Gall and use its folder comparison.
   Check which files differ before copying anything.
3. Download a disposable file in Chromium and choose **Show in folder**.
   Record which manager opened and whether it selected the file. If the system
   opened Nautilus, try `gall --select /path/to/download` on a build that
   supports it and record the two outcomes separately.

At the end of the week, tell the maintainer which task Gall made easier,
when you reopened Nautilus, and which manager you would choose as your default.
Include the Omarchy version, Gall version, and architecture. Use disposable
paths in reports and screenshots.

## Maintainer checklist

- Record the folder association plus D-Bus owner and activation files using
  `scripts/check-omarchy-file-manager.sh gall.desktop`. Repeat with Nautilus
  selected and with each app opened first. In a fresh session trigger Chromium
  reveal before launching either manager and record the resulting owner.
- Verify launcher and terminal-directory shortcut, Dropbox reveal, USB/MTP/SMB,
  default editor, cross-app clipboard, overwrite and trash/undo. Test the
  old and new default after an upgrade and again after rollback/uninstall.
- Compare Gall and Nautilus on the same hardware and fixture, with at least
  20 trials per app for cold start, warm open and reveal. Record median and
  p95 time to an interactive folder and selected item where applicable. Gate
  warm open and reveal on no more than 200 ms p95 behind Nautilus. Check that
  an unreachable SMB share leaves local navigation usable.
- Before proposing the default, record at least 12 seven-day participants,
  including four new to Gall, with at least nine choosing to keep it.
  Require zero outstanding data-loss, wrong-target or rollback defects.
  Record the build versions, screenshots and a maintainer for pilot reports.
