# Themes

A Gall theme is one CSS file that overrides the app's colour variables. You don't need to build
anything or restart the app.

## Included themes

Gall ships with two themes. Pick them in *Settings › Appearance › Custom theme*; there is
nothing to install.

| Theme | Dark mode | Light mode |
|---|---|---|
| Catppuccin | Mocha | Latte |
| Gruvbox | Dark (hard) | Light (soft) |

Gall uses the version that matches *Theme: Dark*, *Light* or *System*. Their source is in
[`ui/src/themes`](../../ui/src/themes), and it's a good reference for a theme of your own.

## Install another theme

This folder has one example to install: [`dracula.css`](dracula.css), the
[Dracula](https://draculatheme.com) palette, one of the most widely used themes on Linux.

1. Copy the `.css` file into the `themes` folder in your settings folder. The quickest way there
   is the folder link under *Settings › Appearance › Custom theme*: it creates the folder if
   needed and opens it in Gall (Shift+click: in the system file manager). The default locations are:

   | System | Folder |
   |---|---|
   | Linux | `~/.config/gall/themes/` |
   | Windows | `%APPDATA%\gall\themes\` |
   | macOS | `~/Library/Application Support/gall/themes/` |
   | Portable | `UserData\themes\` next to `gall.exe` |

2. In *Settings › Appearance › Custom theme*, pick the theme from the dropdown. Files from the
   folder are listed under *Themes folder*. The folder is read again each time you open the
   dropdown, so a file you just copied shows up without a restart.

To go back to the built-in colours, pick *None (built-in colours)*.

A file named like an included theme (for example `catppuccin.css`) takes its place. That's the
way to adjust an included theme: copy it from `ui/src/themes` into the folder and edit it.

## Write your own

Start from a copy of [`dracula.css`](dracula.css), rename it, and change the colours.
Press **`Ctrl+Shift+F5`** in Gall to reload the theme after every save, so you see the change at once.

A theme has up to three blocks per mode:

```css
/* 1. Main colours */
:root:root[data-theme="dark"] {
  --accent: #7aa2f7 !important;
  --bg: #16161e;
  --bg-2: #1a1b26;
  /* … */
}

/* 2. Syntax colours (compare view, source previews) */
:root:root[data-theme="dark"] {
  --sx-keyword: #bb9af7;
  /* … */
}

/* 3. Optional: the GALL wordmark in the title bar */
:root:root[data-theme="dark"] .wordmark {
  background-image: linear-gradient(100deg, #7aa2f7, #c0caf5, #bb9af7);
}
```

Repeat the blocks with `[data-theme="light"]` for light mode. If your theme has only one
version, write just the mode you use, as `dracula.css` does. The other mode keeps Gall's own
colours. The included themes show both modes in one file.

Two details matter:

- **Write the selector as `:root:root[data-theme="dark"]`.** The doubled `:root` gives the rule a
  higher priority than Gall's own stylesheet, whatever order they load in.
- **Add `!important` to `--accent`.** The accent picked in *Settings › Appearance* is applied
  directly to the page, and only `!important` overrides it. Without `!important` your theme
  uses the accent from Settings.

## Variables

### Surfaces and text

| Variable | Used for |
|---|---|
| `--bg` | The canvas behind the panes (the darkest surface in dark mode) |
| `--bg-2` | Panes, sidebar, tab bar |
| `--bg-3` | Raised controls: buttons, inputs, chips |
| `--bg-hover` | Controls under the mouse |
| `--hover` | Row hover overlay (use a transparent colour) |
| `--fg` | Main text |
| `--fg-dim` | Secondary text: dates, sizes, labels |
| `--fg-faint` | Hints, placeholders, disabled items |
| `--border` | Pane frames and dividers |
| `--border-soft` | Hairlines between rows (use a transparent colour) |
| `--glass` | Unused since pop-ups became solid (`--bg-2`); kept so older themes still load |
| `--shadow`, `--shadow-sm` | Shadows of pop-ups and small controls |

### Accent

| Variable | Used for |
|---|---|
| `--accent` | Selection, focus, active pane, links, progress (needs `!important`) |
| `--brand-b` | Second colour of the gradient in the active folder header and the disk usage bars |
| `--accent-2` | Second colour of the gradient border around the focused pane |
| `--sel`, `--sel-inactive` | Selected text and selected rows of the folder compare. Selected items in the file list, sidebar and other lists use `--sel-gradient` and `--sel-bar` (see *Size and shape*). All are computed from `--accent`, so you rarely need to set them. |

### Status and highlights

| Variable | Used for |
|---|---|
| `--danger` | Errors, deleted lines in compare |
| `--ok` | Success, added lines in compare |
| `--warn` | Warnings, changed cells in compare |
| `--mark`, `--mark-fg` | Highlighted search matches (background and text) |
| `--folder` | Fill of folder icons (the back panel is a darker shade of it); default: a pastel of `--accent` |
| `--sm-cpu`, `--sm-gpu`, `--sm-mem` | System monitor rings |

### Syntax colours (compare view, source previews)

Set these on the root, as in block 2 above (themes that set them on `.diffv` colour only the compare view):

| Variable | Colours |
|---|---|
| `--sx-comment` | Comments |
| `--sx-string` | Strings |
| `--sx-number` | Numbers and constants |
| `--sx-keyword` | Keywords |
| `--sx-operator` | Operators and punctuation |
| `--sx-function` | Function names |
| `--sx-type` | Types and classes |
| `--sx-variable` | Variables and tags |
| `--sx-attr` | Attributes |

### Size and shape

These work in a theme too, but most people change them in *Settings › Appearance* instead:
`--gap` (space between panes), `--frame` (focused pane border width), `--font` and `--mono`
(fonts), and the corner rounding:

| Variable | Rounds | Default |
|---|---|---|
| `--radius-sm` | Small marks inside a row, file rows, tiny icon buttons | `6px` |
| `--radius` | Buttons, inputs, tabs, sidebar and list items | `9px` |
| `--radius-lg` | Cards, panes, menus | `12px` |
| `--radius-xl` | Dialogs and windows (Settings, Quick Open, compare, Git) | `16px` |
| `--radius-pill` | Badges and chips | `999px` |

For a square look set all five to a small value, for example `2px`.

Buttons, selections and bars are drawn from shared variables as well, so a theme can restyle
them everywhere at once: `--btn-bg` and `--btn-bg-hover` (button fills), `--btn-shadow`,
`--primary-gradient` (main action), `--accent-text` (accent used for text and icons, mixed
with the foreground so it stays readable), `--sel-gradient` and `--sel-bar` (selected item),
`--row-hover`, `--bar-bg` (window headers and toolbars), `--foot-bg` (footers), `--well-bg`
(track of segmented controls) and `--panel-bg` (dialogs).

## Limits

- **Some colours can't be themed:** the age badges (colour by how old a file is), the colour
  tags and the folder colours come from code.
- **Fonts and icons:** a theme can set `--font`, but icons stay Nerd Font glyphs or Catppuccin SVG icons.
- **Anything else in CSS works too.** You can style any element beyond the variables, but
  class names may change between versions. Variables are the stable part.

## Share your theme

Made a theme you like? Open a pull request that adds it to this folder, with a comment at the
top naming the palette and its licence, as the examples do.
