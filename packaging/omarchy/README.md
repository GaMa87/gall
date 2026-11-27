# Making Gall Omarchy's default file manager

First make Gall available as an optional package, then verify
[the co-install pilot](pilot.md). The upstream
[file-manager selector PR](https://github.com/omacom/omarchy/pull/10542) is the
integration point for folder and terminal launchers. The recipe is
`packaging/arch/gall/PKGBUILD`; copy it and
`packaging/omarchy-pkgs/package.json` to `pkgbuilds/gall/PKGBUILD` and
`pkgbuilds/gall/.omarchy/package.json` in the package repository. Verify both
architectures before submitting.

The pinned v0.2.2 public release supports basic folder launch. The full pilot
requires a reviewed public release with the newer CLI, FileManager1 and
Omarchy integration. Keep the co-install package free of a shared-name
activation file. Before proposing the default, verify an upstream-supported
way to route FileManager1 to the chosen app with both packages installed.
MIME selection does not control the D-Bus owner.

## First frame of a new Gall window

WebKitGTK can submit a blank buffer when a hidden window is first mapped and the
compositor assigns its tiled size. Turning off Hyprland's `fadeIn` alone still
leaves those intermediate buffers. On Hyprland with Wayland presentation
feedback available, Gall uses a temporary startup title with an in-memory rule
that suppresses the window's blur, shadow and opening animation during the first
draw. Once the prepared frame has been presented, the temporary title is
removed and the window uses the desktop's normal rules. Unsupported sessions
and failed startup handshakes use the normal reveal.

To reproduce or compare builds, run the following on a Hyprland output with a
capture region inside the eventual window and over static content:

```sh
scripts/selftest-window-open-hyprland.sh target/debug/gall window solid 'X,Y WIDTHxHEIGHT' /tmp/gall-window-capture
```

The capture also accepts `picker` and `compare` scenarios and `glassy` appearance.
It records monitor scale, compositor settings and events, and a cropped image
sequence. The Wayland bridge builds when the target GTK and Wayland development
files, `wayland-protocols` and `wayland-scanner` are available.

## Candidate fresh-install changes after the pilot

Recheck the paths below against current upstream. Keep Nautilus available
during the first rollout. Existing installations keep their selected manager,
dock entries and custom bindings.

Gall's Hyprland window class is `gall` (verified with `hyprctl clients`), its desktop file is
`gall.desktop`, and `gall --new-window [folder]`, `gall --select <item>` and
`org.freedesktop.FileManager1` provide entry points for testing with a newer Gall build.

1. **`install/omarchy-base.packages`**: add `gall`. Keep `gvfs-mtp`, `gvfs-nfs`, `gvfs-smb` (56-58) and `udiskie` (147).
2. **`default/applications/mimeapps.list`** line 2: `inode/directory=gall.desktop`.
3. **Launchers**: use the upstream selector to dispatch to the selected manager. The Gall branch can use these commands in `bin/omarchy-launch-file-manager` and `bin/omarchy-launch-file-manager-cwd`
   (`# omarchy:summary=...` header like the Nautilus ones):
   ```bash
   exec setsid uwsm-app -- gall --new-window
   exec setsid uwsm-app -- gall --new-window "$(omarchy-cmd-terminal-cwd)"
   ```
   Keep `omarchy-launch-nautilus` and `-cwd` as one-line wrappers that `exec` the new launchers, so
   user bindings and menus keep working.
4. **`default/hypr/bindings/applications.lua`** lines 4-5: `{ omarchy = "file-manager" }` and
   `{ omarchy = "file-manager-cwd" }`.
5. **Window rules** (`org\\.gnome\\.Nautilus` becomes `gall`):
   `default/hypr/windows.lua` line 23 (transparency), `default/hypr/apps/system.lua` line 18
   (floating list), `default/hypr/toggles/window-glassy.lua` line 8 (Glassy, `opacity = "0.75 0.75"`).
6. **`config/omarchy/shell.json`** line 74: pinned dock entry `"org.gnome.Nautilus"` becomes `"gall"`.
7. **`shell/plugins/panels/dropbox/Service.qml`** line 132: route through the selected manager. The Gall route can use
   `["uwsm-app", "--", "gall", "--select", fileUri(String(file.path))]`.
8. **`config/omarchy/extensions/omarchy-menu.jsonc`** line 22 (commented example): `gall ~/Documents`.
9. **`default/gtk/glass.css`** comment mentions Nautilus: Gall draws its own Glassy surfaces from
    the Omarchy theme, nothing to change besides the wording.
10. **Theming (optional)**: Gall follows `~/.local/state/omarchy/current/theme/colors.toml` live. A
    theme may add `gall.css` next to it; for user templates add `default/themed/gall.css.tpl`.
11. **Upgrade**: retain existing manager choices and pinned apps. Verify selecting Gall and switching back.

## Optional: Gall as the open / save dialog

A Gall release with portal support also ships `xdg-desktop-portal-gall`, a FileChooser backend: browsers, chat apps
and the like then show Gall's picker (fuzzy search, Miller columns, Vim keys, previews) for "Open file"
and "Save as". It is off until a portals.conf names it, so nothing changes by default. To make it
Omarchy's default, add `org.freedesktop.impl.portal.FileChooser=gall;gtk` to
`default/hypr/hyprland-portals.conf` (see `hyprland-portals.conf.example`; `;gtk` is the fallback). Add the
picker window to the floating rules in `default/hypr/apps/system.lua` if it should float: it has the
class `gall` like every Gall window, and the title of the request.

## Verify on a clean install

- `xdg-open ~` opens Gall; `Super+Shift+F` opens a window on the active workspace, `Super+Alt+Shift+F`
  in the terminal's folder.
- Chromium "Show in folder" and Dropbox open Gall with the file selected
  (`gdbus call --session --dest org.freedesktop.FileManager1 --object-path /org/freedesktop/FileManager1 --method org.freedesktop.FileManager1.ShowItems "['file:///etc/hostname']" ""`).
- `omarchy theme set <dark>` then `<light>`: Gall recolours without a restart; `decoration:rounding`
  0 versus 8 changes its corners; Glassy keeps the text sharp.
- Settings > Updates says updates come from the package manager.
