import { ThemedSelect } from "./ThemedSelect";
// Settings window (C-01, C-04): searchable, applies changes immediately.
import { cloneElement, isValidElement, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/ipc";
import { FolderLink } from "./FolderLink";
import { CompareAiSettings } from "./CompareAiSettings";
import type { DefaultFileManagerStatus, FontFamily, OpenRule, Settings } from "../api/types";
import { platformScopeNote, restoreActionLabel, unavailableExplanation } from "../core/defaultFileManager";
import { ruleLabel } from "../core/openRules";
import { AVAILABLE, languageName, tt } from "../i18n";
import { ACTION_LABELS, DEFAULT_KEYMAP, KEYBOARD_PROFILES, buildKeymap, defaultKeymap, keyboardProfile, normalizeChord, type Action, type KeyboardProfile } from "../core/keymap";
import * as A from "../state/actions";
import { getState, useStore } from "../state/store";
import { CUSTOM_SCOPE, GIT_RESERVED, GIT_SCOPE_LABELS, builtinKeys, gitCommandList, gitKeyName, parseGitKey } from "../core/gitCommands";
import { CUSTOM_CONTEXTS, CUSTOM_VARIABLES, checkDefinition, joinCommandLine, splitCommandLine, type GitCommandDef, type GitCustomContext } from "../core/gitCustom";
import { useKeyLabels } from "../state/keyLabels";
import { AboutInfo } from "./AboutInfo";
import { Updates } from "./Updates";
import { join } from "../core/path";
import { BUILTIN_THEMES } from "../themes";
import { hasContrastLimits } from "../core/themeContrast";
import { Icon } from "./Icon";
import { GROUP_LABEL, GROUP_ORDER_LABEL, VIEWS } from "./FolderPane";
import { GROUP_MODES, GROUP_ORDERS } from "../core/sortGroup";
import { worktreeLocation, type WorktreeLocation } from "../state/gitWorktree";

type Opt = OptBase & { when?: "omarchy" };
type OptBase =
  /** `action`: its keys in the current mode (vim / chords) are shown as the hint. */
  | { key: keyof Settings; label: string; hint?: string; type: "bool"; action?: Action }
  | { key: keyof Settings; label: string; hint?: string; type: "number"; min?: number; max?: number; step?: number }
  | { key: keyof Settings; label: string; hint?: string; type: "text"; placeholder?: string }
  | { key: keyof Settings; label: string; hint?: string; type: "select"; options: [string, string][] }
  | { key: keyof Settings; label: string; hint?: string; type: "theme" }
  | { key: keyof Settings; label: string; hint?: string; type: "openRules" }
  | { key: keyof Settings; label: string; hint?: string; type: "worktreeLocation" }
  | { key: keyof Settings; label: string; hint?: string; type: "gitCommands" }
  | { key: keyof Settings; label: string; hint?: string; type: "color" }
  | { key: keyof Settings; label: string; hint?: string; type: "list" }
  | { key: keyof Settings; label: string; hint?: string; type: "font" };

type Section = { id: string; title: string; icon: string; desc: string; groups: { title: string; opts: Opt[] }[] };

const SECTIONS: Section[] = [
  {
    id: "appearance", title: "Appearance", icon: "wand", desc: "Colours, theme, fonts and how items look.", groups: [
      { title: "Language", opts: [
        { key: "language", label: "Language", hint: "Automatic follows the operating system language. English, German, Spanish and Slovenian are fully translated.", type: "select", options: [["auto", "Automatic"], ...AVAILABLE.map((c): [string, string] => [c, languageName(c)])] },
      ] },
      { title: "Theme", opts: [
        { key: "theme", label: "Theme", type: "select", options: [["system", "Follow system"], ["dark", "Dark"], ["light", "Light"]] },
        { key: "accent", label: "Accent colour", hint: "Not used while the Omarchy theme is selected: the accent comes from the Omarchy theme.", type: "color" },
        { key: "omarchyFollowFont", label: "Use Omarchy's monospace font", hint: "When no font is chosen below, follow the font set with `omarchy font set`.", type: "bool", when: "omarchy" },
        { key: "customTheme", label: "Custom theme", hint: "Included themes and CSS files in <settings>/themes. Ctrl+Shift+F5 reloads the theme while you edit it. Contrast is adjusted for theme colour variables and gradients; custom images, direct CSS colour overrides and element opacity stay under the stylesheet's control.", type: "theme" },
        { key: "thinScrollbars", label: "Thin scrollbars", type: "bool" },
        { key: "animations", label: "Animations", type: "bool" },
      ] },
      { title: "Items", opts: [
        { key: "fontFamily", label: "Font", hint: "Any installed font, fixed-width or proportional. Code and text previews keep a fixed-width font.", type: "font" },
        { key: "fontStyle", label: "Font style", type: "select", options: [["regular", "Regular"], ["bold", "Bold"], ["italic", "Italic"], ["boldItalic", "Bold italic"]] },
        { key: "fontSize", label: "Font size", type: "number", min: 9, max: 24, step: 0.5 },
        { key: "rowPadding", label: "Row padding (px)", type: "number", min: 0, max: 12 },
        { key: "showNavColumns", label: "Miller columns above the list", hint: "Default for both browsers in the Standard layout; off shows a breadcrumb bar instead (Columns starts with columns, Single with breadcrumbs). The button in a tab bar (or Toggle columns / breadcrumbs in the key sheet) switches one browser in any layout, and that choice is remembered per layout.", type: "bool" },
        { key: "showDetailsPane", label: "Show preview & details pane", hint: "Default for both browsers. Alt+P or the pane button in a tab bar shows or hides it in that browser only; swapping the folders (⇄ in the title bar) keeps the panes. The preview window opens either way.", type: "bool" },
        { key: "marchingAnts", label: "Dotted outline around the keyboard-focused item", type: "bool" },
        { key: "folderSizesAuto", label: "Calculate folder sizes automatically", hint: "Counts every folder in the open folder in the background and shows the total in the Size column (sorting by size then sorts folders too). Off: Ctrl+Alt+S or the context menu calculates on request. Mounted drives inside are not counted.", type: "bool" },
        { key: "showGitStatus", label: "Git status", hint: "In a Git repository: M modified, S staged, U new, ! conflict next to names (folders show what is inside), ignored items dimmed, and the branch in the status bar. Updates when the folder changes and when you come back to the window.", type: "bool" },
        { key: "ageSaturation", label: "Age colour saturation", type: "number", min: 0, max: 1, step: 0.05 },
        { key: "ageLightness", label: "Age colour lightness", type: "number", min: 0, max: 1, step: 0.05 },
        { key: "folderBackgroundImages", label: "Show folder background image", hint: "cover.jpg, folder.jpg, front.jpg, background.jpg or poster.jpg", type: "bool" },
      ] },
    ],
  },
  {
    id: "view", title: "View", icon: "view", desc: "What each folder shows and how it is sorted.", groups: [
      { title: "Visibility", opts: [
        { key: "showHidden", label: "Show hidden files", hint: "Default for both browsers. Press . (or Ctrl+H) to toggle the active browser only, or Ctrl+. / Ctrl+Shift+H for both.", type: "bool", action: "toggleHiddenAll" },
        { key: "hideTemp", label: "Hide temporary files", hint: "~, .tmp, .swp, .bak, .part…", type: "bool" },
      ] },
      { title: "Sorting defaults", opts: [
        { key: "group", label: "Grouping", hint: "What is collected before the sort: folders, files of one type, or both. The sort then orders each group.", type: "select", options: GROUP_MODES.map((m) => [m, GROUP_LABEL[m]]) },
        { key: "groupOrder", label: "Order of the groups", type: "select", options: GROUP_ORDERS.map((o) => [o, GROUP_ORDER_LABEL[o]]) },
        { key: "groupByKind", label: "Kinds (images, documents…) instead of extensions", type: "bool" },
        { key: "groupHeaders", label: "Group headers", hint: "Name, number of files and total size above each group; a click collapses it. Off: a flat list in the same order.", type: "bool" },
        { key: "foldersByName", label: "Always sort folders by name", type: "bool" },
        { key: "defaultSort", label: "Default sort", type: "select", options: [["name", "Name"], ["ext", "Type"], ["modified", "Date modified"], ["created", "Date created"], ["size", "Size"]] },
        { key: "defaultDescending", label: "Descending by default", type: "bool" },
        { key: "defaultView", label: "Default file view", type: "select", options: VIEWS.map((v) => [v.id, v.label]) },
      ] },
      { title: "Folder memory", opts: [
        { key: "rememberFolderViews", label: "Remember view and sort per folder", hint: "On: a sort or view chosen in a folder is kept for it (the view for the folders below too). Off: each browser keeps its own sort and view in every folder it opens. What folders kept comes back when this is on again.", type: "bool" },
      ] },
    ],
  },
  {
    id: "search", title: "Search", icon: "search", desc: "Quick Open index and find in files.", groups: [
      { title: "Quick Open index (Ctrl+P)", opts: [
        { key: "indexRoots", label: "Indexed folders", hint: "Separate with ; — empty = the whole system (/ or every drive). Rebuilt in the background.", type: "list" },
        { key: "indexExclude", label: "Never index or search", hint: "Comma separated names or globs (gitignore syntax), e.g. node_modules, *.iso. Applies to Quick Open, find in files and name search. Folders tagged as caches (CACHEDIR.TAG) are skipped too. Per project: a .gallignore file (gitignore syntax, also outside git) in a folder applies to it and everything below; right-click › Hide from search adds an item to it. Alt+I in a search includes all of these.", type: "list" },
        { key: "indexHidden", label: "Index hidden files and folders", hint: "Also applies to find in files (Ctrl+F)", type: "bool" },
        { key: "indexRespectGitignore", label: "Skip files ignored by .gitignore", type: "bool" },
        { key: "indexRefreshMinutes", label: "Re-index every (minutes)", hint: "0 = only when Quick Open finds the index older than a minute", type: "number", min: 0, max: 1440 },
      ] },
      { title: "Find in files and name search", opts: [
        { key: "searchRespectGitignore", label: "Skip files ignored by .gitignore", type: "bool" },
      ] },
    ],
  },
  {
    id: "keyboard-vim", title: "Keyboard", icon: "code", desc: "Three keyboard profiles with all Gall commands, searchable help and custom shortcuts.", groups: [
      { title: "Keyboard profile", opts: [
        { key: "keyboardProfile", label: "Keyboard profile", hint: "Choose familiar shortcuts; every profile keeps all Gall commands. ? opens this profile's help. Custom shortcuts apply to all profiles.", type: "select", options: Object.entries(KEYBOARD_PROFILES) as [KeyboardProfile, string][] },
      ] },
      { title: "Command menu", opts: [
        { key: "leaderKey", label: "Leader key (Vim)", hint: "Opens the which-key menu. Space Space = go to file, as in LazyVim.", type: "select", options: [["Space", "Space (default)"], ["\\", "\\ (backslash, Vim's default)"], [";", "; (semicolon)"], ["Ctrl", "Ctrl once"], ["Ctrl Ctrl", "Ctrl twice"]] },
        { key: "leaderKeyBasic", label: "Leader key (Explorer / Commander)", hint: "Opens the same menu for all Gall commands. Letters still type to filter; Space previews.", type: "select", options: [["Ctrl Ctrl", "Ctrl twice (default)"], ["Ctrl", "Ctrl once"], ["\\", "\\ (backslash)"], [";", "; (semicolon)"], ["", "None"]] },
      ] },
    ],
  },
  {
    id: "columns", title: "Columns", icon: "columns", desc: "Miller columns: where they start and how wide they are.", groups: [
      { title: "Miller columns", opts: [
        { key: "columnsFromHome", label: "Start columns at the home folder when inside it", hint: "Drives and / stay reachable from the drives column", type: "bool" },
        { key: "columnHoverExpand", label: "Expand a column when the pointer rests on it", type: "bool" },
        { key: "columnExpandDelayMs", label: "Expand delay (ms)", type: "number", min: 0, max: 3000, step: 50 },
        { key: "columnMinWidth", label: "Minimum column width", type: "number", min: 30, max: 300 },
        { key: "columnMaxWidth", label: "Maximum column width", type: "number", min: 120, max: 600 },
      ] },
    ],
  },
  {
    id: "behavior", title: "Behaviour", icon: "activity", desc: "Clicking, dragging and tabs.", groups: [
      { title: "Opening", opts: [
        { key: "singleClickOpen", label: "Open items with a single click", type: "bool" },
        { key: "slowClickRename", label: "Rename by clicking a selected item again", type: "bool" },
      ] },
      { title: "Drag & drop", opts: [
        { key: "dragBehavior", label: "Default drop action", hint: "While dragging, hold Ctrl = copy, Shift = move, Alt = symbolic link. Releasing the key returns to this default.", type: "select", options: [["consistent", "Move"], ["copy", "Copy"], ["system", "Move on same drive, copy across drives"], ["ask", "Ask (show menu)"]] },
      ] },
      { title: "Git commits", opts: [
        { key: "gitBranchWip", label: "Remember changes per branch", hint: "Switching branches with local changes keeps them with the branch you leave and brings them back when you return, instead of asking what to do with them. Changes that do not fit on top of a branch as it is later stay saved and can become a stash. Off by default.", type: "bool" },
        { key: "gitDoubleClick", label: "Double-click on a commit", type: "select", options: [["details", "Show its changes"], ["checkout", "Check it out (asks first)"]] },
        { key: "gitForecastBranches", label: "Conflict forecast up to this many branches", hint: "The branch list shows which branches would conflict when merged into the current one (Git 2.38). Not in repositories with more branches; 0 turns it off.", type: "number", min: 0, max: 1000 },
      ] },
      { title: "Git commands", opts: [
        { key: "gitCommands", label: "Your Git commands", hint: "A program and its arguments, run in the repository without a shell. Variables: {hash} {branch} {tag} {stash} {file} {selection} {repo} {head}. They appear in the menu, the command palette and the key sheet; Settings › Shortcuts can rebind their keys.", type: "gitCommands" },
      ] },
      { title: "Git worktrees", opts: [
        { key: "worktreeLocation", label: "Location of new worktrees", hint: "{repo} is the repository folder (its name after the start), {branch} the branch. ../{repo}-{branch} puts worktrees next to the repository. Repository tools › Worktrees overrides it for one repository.", type: "worktreeLocation" },
      ] },
      { title: "Tabs", opts: [
        { key: "newTabPath", label: "New tab opens", hint: "Empty = copy of current tab. Supports ~ and $VARS.", type: "text", placeholder: "(clone current tab)" },
        { key: "newTabPlacement", label: "New tab position", type: "select", options: [["afterCurrent", "After current tab"], ["end", "At the end"]] },
        { key: "tabsShowFullPath", label: "Show full path on tabs", type: "bool" },
        { key: "startupPaths", label: "Start-up folders (first launch / no saved session)", hint: "Separate with ; — the first for browser 1, the second for browser 2", type: "list" },
      ] },
      { title: "Workspaces & Tab Pairs", opts: [
        { key: "showWorkspaces", label: "Workspace switcher in title bar", hint: "Always shows 1 2 3 +; Only with multiple hides it when just one workspace is open; Never hides it completely.", type: "select", options: [["always", "Always show"], ["multiple", "Only with multiple"], ["never", "Never show"]] },
        { key: "liveTabPairs", label: "Live Tab Pairs bridge", hint: "Show the center bridge control in the title bar and pair actions on tabs (Linked, Tandem walk, Flow). Turning off restores classic independent panels.", type: "bool" },
        { key: "pairGravity", label: "Pair Gravity ghost tab suggestions", hint: "Suggest a translucent ghost tab (🧲) on the opposite tab bar based on learned folder pairs and sibling Git worktrees.", type: "bool" },
      ] },
    ],
  },
  {
    id: "window", title: "Window", icon: "window", desc: "Start-up, sidebar and notifications.", groups: [
      { title: "Start-up", opts: [
        { key: "startMaximized", label: "Open maximized", hint: "Applies the next time Gall starts. Tiling window managers (Hyprland, Sway, Niri) size the window themselves.", type: "bool" },
      ] },
      { title: "Layout and window frame", opts: [
        { key: "adaptiveLayout", label: "Adapt layout to the window size", hint: "Wide window: two browsers side by side. Tall half-screen tile: two stacked. Small tile: one browser, with the sidebar hiding in narrow windows. Choosing a layout yourself or collapsing a browser always wins.", type: "bool" },
        { key: "windowButtons", label: "Window buttons", hint: "Minimize, maximize and close in the title bar. Auto hides them in tiling window managers, which have their own controls.", type: "select", options: [["auto", "Automatic"], ["always", "Always show"], ["never", "Never show"]] },
        { key: "omarchyGlassy", label: "Follow Omarchy Glassy mode", hint: "Translucent surfaces over Hyprland's blur while Omarchy's Glassy toggle is on; text and icons stay sharp. Needs the Omarchy theme.", type: "bool", when: "omarchy" },
      ] },
      { title: "Sidebar", opts: [
        { key: "showDrives", label: "Show drives", type: "bool" },
        { key: "showCloud", label: "Show cloud folders", hint: "OneDrive, Dropbox, Google Drive, iCloud and others found on this computer", type: "bool" },
        { key: "showHistory", label: "Show history", type: "bool" },
        { key: "showSystemMonitor", label: "Show system monitor", hint: "CPU, GPU, memory, temperatures, network and disk activity at the bottom of the sidebar. Steps aside while the window is too short for five History rows and returns when there is room; the toolbar button shows it anyway.", type: "bool" },
        { key: "historyCount", label: "Folders remembered in history", type: "number", min: 5, max: 200 },
      ] },
      { title: "Other", opts: [
        { key: "notificationMs", label: "Notification duration (ms)", type: "number", min: 1000, max: 30000, step: 500 },
        { key: "pathCopyFormat", label: "Default path copy format", type: "select", options: [["native", "As is"], ["nativeQuoted", "Quoted"], ["escaped", "Escaped backslashes"], ["forward", "Forward slashes"], ["forwardQuoted", "Forward slashes, quoted"], ["uri", "file:// URI"], ["linux", "Linux path (WSL on Windows)"]] },
      ] },
    ],
  },
  {
    id: "openWith", title: "Open with", icon: "open", desc: "Terminal, editor and the programs remembered for files.", groups: [
      { title: "Terminal", opts: [
        { key: "terminal", label: "Terminal command (F10)", hint: "Easier: Ctrl+F10 (Terminal here with…), pick one, Ctrl+Enter. Empty = auto-detect. {dir} = the folder; quote a program path with spaces.", type: "text", placeholder: "auto" },
      ] },
      { title: "Editor", opts: [
        { key: "editor", label: "Editor command (Edit in editor)", hint: "Easier: Shift+F4 (Edit with…), pick one, Ctrl+Enter. Empty = ask the first time. {path} = the file.", type: "text", placeholder: "ask" },
      ] },
      { title: "Programs for files", opts: [
        { key: "openRules", label: "Remembered programs", hint: "Made with Ctrl+Enter in Open with…. The most specific rule wins: one file, then the deepest folder, then everywhere. {path} = the file.", type: "openRules" },
      ] },
    ],
  },
  {
    id: "preview", title: "Preview", icon: "image", desc: "Preview window, thumbnails and text excerpts.", groups: [
      { title: "Preview", opts: [
        { key: "autoplayMedia", label: "Autoplay audio/video in the preview window", type: "bool" },
        { key: "videoSeek", label: "←/→ in video jumps by", type: "select", options: [["fraction", "Fraction of length"], ["seconds", "Seconds"]] },
        { key: "videoSeekFraction", label: "Fraction (1/n of length)", type: "number", min: 2, max: 100 },
        { key: "videoSeekSeconds", label: "Seconds", type: "number", min: 1, max: 600 },
        { key: "previewTextLimit", label: "Max text preview size (bytes)", type: "number", min: 1024, max: 50000000, step: 1024 },
      ] },
      { title: "Thumbnails & excerpts", opts: [
        { key: "noThumbnailExts", label: "Extensions without thumbnails", hint: "Comma separated", type: "list" },
        { key: "excerptExts", label: "Extensions showing text excerpts", hint: "Thumbnails + details view", type: "list" },
        { key: "excerptLines", label: "Excerpt lines", type: "number", min: 1, max: 10 },
      ] },
    ],
  },
  {
    id: "advanced", title: "Advanced", icon: "settings", desc: "Rarely needed knobs.", groups: [
      { title: "File watching", opts: [{ key: "watcherDebounceMs", label: "Change debounce (ms, restart required)", type: "number", min: 50, max: 5000, step: 50 }] },
      { title: "Comparison", opts: [{ key: "generatedPatterns", label: "Extra generated file patterns", hint: "Comma separated names or globs (e.g. *.pb.go, gen/*.rs) collapsed by default in comparisons and Git file lists.", type: "list" }] },
    ],
  },
];

/** A field whose text is parsed into the setting: edited freely and saved on Enter or
 *  when left, not on every key ("1" on the way to "16" would set a 1 px font; a list
 *  would drop the comma just typed). `commit` returns false for text it cannot use, which
 *  then goes back to the saved value. Esc reverts. */
function DraftInput({ saved, commit, live, ...rest }: {
  saved: string; commit: (text: string) => boolean;
  /** Text that may be saved while typing too (a number in range: the spin arrows act at once). */
  live?: (text: string) => boolean;
} & Omit<React.InputHTMLAttributes<HTMLInputElement>, "value" | "onChange">) {
  const [text, setText] = useState(saved);
  useEffect(() => setText(saved), [saved]);
  const save = () => {
    if (text === saved) return;
    if (!commit(text)) setText(saved);
  };
  // Settings closed by a click outside: the field goes away without a blur; save it then.
  const latest = useRef({ text, saved, commit });
  latest.current = { text, saved, commit };
  useEffect(() => () => { const l = latest.current; if (l.text !== l.saved) l.commit(l.text); }, []);
  return (
    <input {...rest} value={text} onChange={(e) => { setText(e.target.value); if (live?.(e.target.value)) commit(e.target.value); }} onBlur={save}
      onKeyDown={(e) => {
        if (e.key === "Enter") { e.preventDefault(); save(); }
        if (e.key === "Escape" && text !== saved) { e.preventDefault(); e.stopPropagation(); setText(saved); }
      }} />
  );
}

function OptRow({ o, s }: { o: Opt; s: Settings }) {
  const omarchyActive = useStore((st) => !!st.omarchy?.active);
  // "Omarchy" is offered where Omarchy runs (and stays visible if it is already chosen).
  if (o.key === "theme" && o.type === "select" && (omarchyActive || s.theme === "omarchy") && !o.options.some(([k]) => k === "omarchy")) {
    o = { ...o, options: [...o.options, ["omarchy", "Omarchy (live desktop theme)"]] };
  }
  const v = o.key === "keyboardProfile" ? keyboardProfile(s) : s[o.key] as unknown;
  const set = (val: unknown) => A.updateSettings({ [o.key]: val } as Partial<Settings>, o.type === "text" || o.type === "color");
  const keys = useKeyLabels();
  if (o.when === "omarchy" && !omarchyActive) return null;
  let ctl: React.ReactNode;
  switch (o.type) {
    case "bool": ctl = <input type="checkbox" className="switch" role="switch" title={tt(o.label)} checked={!!v} onChange={(e) => set(e.target.checked)} />; break;
    case "number": ctl = (
      <DraftInput type="number" saved={String(v)} min={o.min} max={o.max} step={o.step ?? 1}
        live={(t) => String(Number(t)) === t.trim() && Number(t) >= (o.min ?? -Infinity) && Number(t) <= (o.max ?? Infinity)}
        commit={(t) => {
          const n = Number(t);
          if (t.trim() === "" || !Number.isFinite(n)) return false;
          set(Math.min(o.max ?? Infinity, Math.max(o.min ?? -Infinity, n)));
          return true;
        }} />
    ); break;
    case "text": ctl = <input type="text" value={v as string} placeholder={o.placeholder && tt(o.placeholder)} onChange={(e) => set(e.target.value)} spellCheck={false} />; break;
    case "select": ctl = <ThemedSelect value={v as string} onChange={(e) => set(e.target.value)}>{o.options.map(([k, l]) => <option key={k} value={k}>{tt(l)}</option>)}</ThemedSelect>; break;
    case "theme": ctl = <ThemePicker value={v as string} set={set} />; break;
    case "font": ctl = <FontPicker value={v as string} set={set} />; break;
    case "worktreeLocation": ctl = <input type="text" value={v as string} placeholder="{repo}/.worktrees/{branch}" onChange={(e) => A.updateSettings({ worktreeLocation: e.target.value }, true)} spellCheck={false} />; break;
    case "gitCommands": return <GitCommandsList commands={v as GitCommandDef[]} set={set} label={tt(o.label)} hint={o.hint && tt(o.hint)} />;
    case "openRules": return <OpenRulesList rules={v as OpenRule[]} set={set} label={tt(o.label)} hint={o.hint && tt(o.hint)} />;
    case "color": ctl = <input type="color" value={v as string} onChange={(e) => set(e.target.value)} />; break;
    case "list": {
      // Paths may contain commas: those lists are separated by ";".
      const paths = o.key === "startupPaths" || o.key === "indexRoots";
      ctl = (
        <DraftInput type="text" saved={(v as string[]).join(paths ? " ; " : ", ")} spellCheck={false}
          commit={(t) => { set(t.split(paths ? ";" : ",").map((x) => x.trim()).filter(Boolean)); return true; }} />
      );
      break;
    }
  }
  // A toggle with a key: say it is a key ("." alone looked like a stray dot).
  const key = o.type === "bool" && o.action ? keys.action(o.action) : null;
  const hint = key ? `${tt("Shortcut:")} ${key}${o.hint ? ` · ${tt(o.hint)}` : ""}` : o.hint && tt(o.hint);
  const id = `opt-${o.key}`;
  return (
    <div className={`opt ${o.type === "bool" ? "toggle" : ""}`}>
      <label htmlFor={id} className="opt-text"><span className="opt-label">{tt(o.label)}</span>{hint && <small>{hint}</small>}{o.type === "worktreeLocation" && <WorktreePreview template={v as string} />}</label>
      <div className="opt-ctl">{withId(ctl, id)}</div>
    </div>
  );
}

/** The folder a worktree of the branch `feature/example` would get in the open folder's repository. */
function WorktreePreview({ template }: { template: string }) {
  const path = useStore((s) => { const b = s.session?.browsers[s.session.activeBrowser]; return b?.tabs[b.active]?.path ?? ""; });
  const [place, setPlace] = useState<WorktreeLocation | null>(null);
  useEffect(() => {
    let active = true;
    const timer = window.setTimeout(() => { void worktreeLocation(path, "feature/example", template, {}).then((value) => { if (active) setPlace(value); }).catch(() => { if (active) setPlace(null); }); }, 120);
    return () => { active = false; window.clearTimeout(timer); };
  }, [path, template]);
  if (!place) return null;
  return <small className={place.error ? "opt-preview-error" : undefined} role="status">{place.error ?? tt("Preview: {path}", { path: place.path ?? "" })}</small>;
}

/** Gives the row's control the id its label points at (clicking the label focuses / toggles it). */
function withId(ctl: React.ReactNode, id: string): React.ReactNode {
  return isValidElement(ctl) && (ctl.type === "input" || ctl.type === ThemedSelect) ? cloneElement(ctl as React.ReactElement<{ id?: string }>, { id }) : ctl;
}

/** Theme files (<config>/themes/*.css) as a dropdown; the list is re-read when the menu is opened. */
/** Installed fonts in two groups (fixed-width, proportional); scanned when first opened. */
function FontPicker({ value, set }: { value: string; set: (v: string) => void }) {
  const [fonts, setFonts] = useState<FontFamily[] | null>(null);
  const load = () => { if (!fonts) api.fontFamilies().then(setFonts).catch(() => setFonts([])); };
  useEffect(load, []); // eslint-disable-line react-hooks/exhaustive-deps
  const mono = fonts?.filter((f) => f.monospace) ?? [];
  const prop = fonts?.filter((f) => !f.monospace) ?? [];
  const known = !value || fonts?.some((f) => f.name === value);
  const cur = fonts?.find((f) => f.name === value);
  return (
    <span className="font-pick">
      <ThemedSelect value={value} onFocus={load} onChange={(e) => set(e.target.value)} style={value ? { fontFamily: `"${value}"` } : undefined}>
        <option value="">{tt("Default (JetBrains Mono)")}</option>
        {!known && <option value={value}>{value} {fonts ? tt("(not installed)") : ""}</option>}
        {fonts == null && <option disabled>{tt("Loading fonts…")}</option>}
        {mono.length > 0 && <optgroup label={tt("Fixed width")}>{mono.map((f) => <option key={f.name} value={f.name}>{f.name}</option>)}</optgroup>}
        {prop.length > 0 && <optgroup label={tt("Proportional")}>{prop.map((f) => <option key={f.name} value={f.name}>{f.name}</option>)}</optgroup>}
      </ThemedSelect>
      {cur && (!cur.bold || !cur.italic) && <small className="font-note">{!cur.bold && !cur.italic ? tt("no bold or italic face: imitated") : !cur.bold ? tt("no bold face: imitated") : tt("no italic face: imitated")}</small>}
    </span>
  );
}

function ThemePicker({ value, set }: { value: string; set: (v: string) => void }) {
  const info = useStore((s) => s.info);
  const contrastReport = useStore((s) => s.themeContrastReport);
  const dir = info ? join(info.configDir, "themes") : "";
  const [names, setNames] = useState<string[]>([]);
  const refresh = () => {
    if (!dir) return;
    api.listDir(dir, { showHidden: false, hideTemp: true })
      .then((es) => setNames(es.filter((e) => !e.isDir && e.ext === "css").map((e) => e.name.slice(0, -4)).sort((a, b) => a.localeCompare(b))))
      .catch(() => setNames([])); // no themes folder yet
  };
  useEffect(refresh, [dir]);
  // The folder may not exist yet (no themes added): create it first.
  const ensureDir = () => (info ? api.createDir(info.configDir, "themes") : Promise.resolve());
  const builtins = Object.entries(BUILTIN_THEMES);
  const files = names.filter((n) => !BUILTIN_THEMES[n]);
  const missing = value && !names.includes(value) && !BUILTIN_THEMES[value];
  const limitNotes: string[] = [];
  if (contrastReport && hasContrastLimits(contrastReport)) {
    if (contrastReport.imageSurfaces.length > 0) {
      limitNotes.push(tt("Image backgrounds ({vars}) cannot be checked automatically.", { vars: contrastReport.imageSurfaces.join(", ") }));
    }
    if (contrastReport.mixedGradientSurfaces.length > 0) {
      limitNotes.push(tt("Gradients with opposing light and dark stops ({vars}) have no single 4.5:1 text colour.", { vars: contrastReport.mixedGradientSurfaces.join(", ") }));
    }
    if (contrastReport.customForegroundVars.length > 0) {
      limitNotes.push(tt("Custom foreground overrides ({vars}) bypass automatic contrast and fall below 4.5:1.", { vars: contrastReport.customForegroundVars.join(", ") }));
    }
    if (contrastReport.hasCustomCssRules || contrastReport.hasOpacityRules) {
      limitNotes.push(tt("Custom CSS colour or opacity rules outside :root are not rewritten automatically."));
    }
    if (contrastReport.translucentWindow) {
      limitNotes.push(tt("Translucent window backgrounds depend on what sits behind Gall."));
    }
  }
  return (
    <span className="theme-pick">
      <ThemedSelect value={value} onFocus={refresh} onMouseDown={refresh} onChange={(e) => set(e.target.value)}>
        <option value="">{tt("None (built-in colours)")}</option>
        <optgroup label={tt("Included")}>
          {builtins.map(([k, t]) => <option key={k} value={k}>{t.label}{names.includes(k) ? ` ${tt("(your file)")}` : ""}</option>)}
        </optgroup>
        {files.length > 0 && <optgroup label={tt("Themes folder")}>{files.map((n) => <option key={n} value={n}>{n}</option>)}</optgroup>}
        {missing && <option value={value}>{value} {tt("(file not found)")}</option>}
      </ThemedSelect>
      <FolderLink path={dir} before={ensureDir} />
      {limitNotes.length > 0 && <small className="theme-contrast-note" role="status">{limitNotes.join(" ")}</small>}
    </span>
  );
}

/** Remembered "always open with" rules: where each applies, the program, its command. */
function OpenRulesList({ rules, set, label, hint }: { rules: OpenRule[]; set: (v: OpenRule[]) => void; label: string; hint?: string }) {
  const home = useStore((s) => s.info?.home ?? "");
  // Broad to narrow, like the palette's scopes; same type together.
  const sorted = [...rules].sort((a, b) => a.target.localeCompare(b.target) || (a.dir ?? "").localeCompare(b.dir ?? ""));
  const update = (r: OpenRule, patch: Partial<OpenRule> | null) =>
    set(patch ? rules.map((x) => (x === r ? { ...x, ...patch } : x)) : rules.filter((x) => x !== r));
  return (
    <div className="opt rules">
      <div className="opt-text"><span className="opt-label">{label}</span>{hint && <small>{hint}</small>}</div>
      {sorted.length === 0 ? (
        <div className="rules-empty">{tt("None yet. Select a file, press O (or Ctrl+Enter), pick a program and press Ctrl ↵.")}</div>
      ) : (
        <div className="rules-list">
          {sorted.map((r) => (
            <div key={r.target + (r.dir ?? "")} className="rule">
              <span className="rule-where" title={r.dir ?? r.target}>{ruleLabel(r, home)}</span>
              <span className="rule-name">{r.name}</span>
              <input type="text" value={r.command} spellCheck={false} title={tt("Command; {path} = the file", { path: "{path}" })} onChange={(e) => update(r, { command: e.target.value })} />
              <button className="icon-btn" title={tt("Forget (use the system default)")} onClick={() => update(r, null)}><Icon name="close" size={12} /></button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** One argument line of a command: typed freely, kept as arguments when it splits (an unclosed quote waits). */
function CommandLineInput({ argv, set, label }: { argv: string[]; set: (argv: string[]) => void; label: string }) {
  const [text, setText] = useState<string | null>(null);
  const bad = text !== null && splitCommandLine(text) === null;
  const commit = () => { if (text === null) return; const parsed = splitCommandLine(text); if (parsed) { set(parsed); setText(null); } };
  return <input type="text" aria-label={label} aria-invalid={bad} value={text ?? joinCommandLine(argv)} spellCheck={false} placeholder="git log --oneline {hash}" onChange={(e) => setText(e.target.value)} onBlur={commit} onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); commit(); } if (e.key === "Escape" && text !== null) { e.preventDefault(); e.stopPropagation(); setText(null); } }} />;
}

/** The Git commands of the user: label, where it applies, the command, an optional key and whether to ask first. */
function GitCommandsList({ commands, set, label, hint }: { commands: GitCommandDef[]; set: (v: GitCommandDef[]) => void; label: string; hint?: string }) {
  const update = (id: string, patch: Partial<GitCommandDef> | null) => set(patch ? commands.map((x) => (x.id === id ? { ...x, ...patch } : x)) : commands.filter((x) => x.id !== id));
  const add = () => set([...commands, { id: `cmd-${Date.now().toString(36)}`, label: "", context: "commit", command: [], key: "", confirm: true }]);
  const explain = (def: GitCommandDef): string => {
    const found = checkDefinition(def, builtinKeys(CUSTOM_SCOPE[def.context]), commands);
    if (!found.problem) return "";
    if (found.problem === "label") return tt("Give it a name.");
    if (found.problem === "command") return tt("Type the program and its arguments.");
    if (found.problem === "key") return tt("A key is one character.");
    if (found.problem === "taken") return tt("The key {key} is already used here.", { key: found.detail ?? "" });
    return tt("{problem} Variables: {names}", { problem: found.detail ?? "", names: CUSTOM_VARIABLES.join(" ") });
  };
  return (
    <div className="opt rules gitcmds">
      <div className="opt-text"><span className="opt-label">{label}</span>{hint && <small>{hint}</small>}</div>
      {commands.length === 0 ? <div className="rules-empty">{tt("None yet. Add one, for example “{example}” on a commit.", { example: "git log --oneline {hash}" })}</div> : (
        <div className="rules-list">
          {commands.map((c) => {
            const problem = explain(c);
            return <div key={c.id} className="gitcmd">
              <input type="text" aria-label={tt("Name of the command")} value={c.label} placeholder={tt("Name")} onChange={(e) => update(c.id, { label: e.target.value })} />
              <ThemedSelect aria-label={tt("Where the command applies")} value={c.context} onChange={(e) => update(c.id, { context: e.target.value as GitCustomContext })}>{CUSTOM_CONTEXTS.map((context) => <option key={context} value={context}>{tt(context)}</option>)}</ThemedSelect>
              <CommandLineInput argv={c.command} set={(command) => update(c.id, { command })} label={tt("Command of {name}", { name: c.label || c.id })} />
              <input type="text" className="gitcmd-key" maxLength={1} aria-label={tt("Key of the command")} value={c.key} placeholder={tt("key")} onChange={(e) => update(c.id, { key: e.target.value.trim() })} />
              <label className="gitcmd-ask"><input type="checkbox" checked={c.confirm} onChange={(e) => update(c.id, { confirm: e.target.checked })} />{tt("Ask first")}</label>
              <button className="icon-btn" title={tt("Remove this command")} onClick={() => update(c.id, null)}><Icon name="close" size={12} /></button>
              {problem && <small className="gitcmd-problem" role="alert">{problem}</small>}
            </div>;
          })}
        </div>
      )}
      <button onClick={add}>{tt("Add a Git command")}</button>
    </div>
  );
}

function Shortcuts({ s, q }: { s: Settings; q: string }) {
  const actions = (Object.keys(DEFAULT_KEYMAP) as Action[]).filter((a) => !q || ACTION_LABELS[a].toLowerCase().includes(q) || tt(ACTION_LABELS[a]).toLowerCase().includes(q));
  const profile = keyboardProfile(s);
  const keymap = useMemo(() => buildKeymap(s.shortcuts, profile), [s.shortcuts, profile]);
  return (
    <>
      {!q && <p className="section-note">{tt("Profile:")} <b>{KEYBOARD_PROFILES[profile]}</b>. {tt("Custom shortcuts apply to all profiles. Comma-separate several chords, e.g. F5, Ctrl+R. Enter or leaving the field saves; an empty field restores this profile's default.")}</p>}
      <div className="scard kb-card">
      {actions.map((a) => {
        const saved = s.shortcuts[a] ?? defaultKeymap(profile)[a].join(", ");
        // A chord of this action that another action has: the other one wins or loses it.
        const clash = saved.split(",").map((c) => normalizeChord(c.trim())).filter(Boolean)
          .map((c) => [c, keymap.get(c)] as const).find(([, other]) => other && other !== a);
        return <ChordRow key={a} id={a} label={tt(ACTION_LABELS[a])} defaults={defaultKeymap(profile)[a].join(", ")} s={s}
          clash={clash && tt("{chord} runs “{action}”", { chord: clash[0], action: tt(ACTION_LABELS[clash[1]!]) })} />;
      })}
      </div>
      {gitGroups(s.shortcuts, q).map(({ scope, commands }) => (
        <section key={scope} className="sgroup">
          <h4>{tt("Git")} · {tt(GIT_SCOPE_LABELS[scope])}</h4>
          <div className="scard kb-card">{commands.map((command) => {
            const reserved = (s.shortcuts[command.id] ?? "").split(",").map((c) => c.trim()).filter(Boolean).map(parseGitKey).find((key) => GIT_RESERVED.includes(key));
            const lost = !reserved && command.keys.length === 0;
            return <ChordRow key={command.id} id={command.id} label={tt(command.label)} defaults={command.defaults.map(gitKeyName).join(", ")} s={s}
              clash={reserved ? tt("{key} moves between rows and panels and cannot run a command", { key: reserved }) : lost ? tt("No key: another command uses it") : undefined} />;
          })}</div>
        </section>
      ))}
      {!q && <p className="section-note">{tt("Git keys are case-sensitive: w and W are two keys. Letters work while Vim keys are on; a chord such as Ctrl+G or F6 works without them too.")}</p>}
    </>
  );
}

/** The Git commands that a search matches, by panel. */
function gitGroups(shortcuts: Record<string, string>, q: string) {
  return gitCommandList(shortcuts)
    .map(({ scope, commands }) => ({ scope, commands: commands.filter((c) => !q || `git ${GIT_SCOPE_LABELS[scope]} ${tt(GIT_SCOPE_LABELS[scope])} ${c.label} ${tt(c.label)}`.toLowerCase().includes(q)) }))
    .filter(({ commands }) => commands.length > 0);
}

/** One action's chords: edited as text and saved on Enter or blur (not on every key, so
 *  clearing the field to type a new chord does not snap back to the default). */
function ChordRow({ id, label, defaults, s, clash }: { id: string; label: string; defaults: string; s: Settings; clash?: string }) {
  const saved = s.shortcuts[id] ?? defaults;
  const [text, setText] = useState(saved);
  useEffect(() => setText(saved), [saved]);
  const commit = (t = text) => {
    const sc = { ...getState().settings!.shortcuts };
    const v = t.trim();
    if (v === "" || v === defaults) delete sc[id];
    else sc[id] = v;
    if (JSON.stringify(sc) !== JSON.stringify(getState().settings!.shortcuts)) A.updateSettings({ shortcuts: sc });
    else setText(saved);
  };
  // Closed by a click outside: no blur comes; save what was typed.
  const latest = useRef({ text, saved, commit });
  latest.current = { text, saved, commit };
  useEffect(() => () => { const l = latest.current; if (l.text !== l.saved) l.commit(l.text); }, []);
  return (
    <div className={`kb-row ${s.shortcuts[id] != null ? "changed" : ""}`}>
      <span>{label}{clash && <span className="kb-clash" title={clash}> · {clash}</span>}</span>
      <input type="text" value={text} spellCheck={false} data-shortcut={id}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => commit()}
        onKeyDown={(e) => {
          if (e.key === "Enter") { e.preventDefault(); commit(); }
          if (e.key === "Escape" && text !== saved) { e.preventDefault(); e.stopPropagation(); setText(saved); }
        }} />
    </div>
  );
}

/** One group of options as a card under its title. */
function Group({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="sgroup">
      <h4>{tt(title)}</h4>
      <div className="scard">{children}</div>
    </section>
  );
}

const EXTRA_NAV = [
  { id: "about", title: "About", icon: "info", desc: "" },
  { id: "updates", title: "Updates", icon: "info", desc: "" },
  { id: "shortcuts", title: "Shortcuts", icon: "list", desc: "Rebind any action. Vim keys and the leader menu are listed in the key sheet." },
];

function FileManagerStatus() {
  const platform = useStore((st) => st.info?.platform ?? "linux");
  const linux = platform === "linux";
  const [request, setRequest] = useState(0);
  const [status, setStatus] = useState<Awaited<ReturnType<typeof api.fileManagerStatus>> | "error" | null>(null);
  const [dfm, setDfm] = useState<DefaultFileManagerStatus | "error" | null>(null);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    api.defaultFileManagerStatus().then(
      (result) => { if (active) setDfm(result); },
      () => { if (active) setDfm("error"); },
    );
    if (linux) {
      api.fileManagerStatus().then(
        (result) => { if (active) setStatus(result); },
        () => { if (active) setStatus("error"); },
      );
    }
    return () => { active = false; };
  }, [linux, request]);

  const refresh = () => {
    setActionError(null);
    setDfm(null);
    if (linux) setStatus(null);
    setRequest((n) => n + 1);
  };

  const apply = async (enable: boolean) => {
    if (busy) return;
    setBusy(true);
    setActionError(null);
    try {
      const next = enable ? await api.defaultFileManagerSet() : await api.defaultFileManagerRestore();
      setDfm(next);
      if (linux) {
        api.fileManagerStatus().then(
          (result) => setStatus(result),
          () => setStatus("error"),
        );
      }
    } catch (err) {
      setActionError(String(err instanceof Error ? err.message : err));
    } finally {
      setBusy(false);
    }
  };

  const folderText = linux
    ? (status === "error" ? tt("Status unavailable") : status === null ? tt("Loading…") : status.folderHandler ?? tt("None"))
    : (dfm === "error" ? tt("Status unavailable") : dfm === null ? tt("Loading…") : dfm.currentHandler ?? tt("None"));
  const revealText = status === "error" || (status && !status.revealAvailable)
    ? tt("Status unavailable") : status === null ? tt("Loading…") : status.revealHandler ?? tt("None");
  const dfmOk = dfm && dfm !== "error" ? dfm : null;
  const activePlatform = dfmOk?.platform ?? platform;
  const unavailable = dfmOk && !dfmOk.canSet && !dfmOk.isDefault ? unavailableExplanation(dfmOk.unavailableReason, tt) : null;

  return <Group title="File manager integration">
    <div className="opt">
      <div className="opt-text">
        <span className="opt-label">{tt("Default for opening folders")}</span>
        <small data-file-manager-folder role="status">{folderText}</small>
        <small>{platformScopeNote(activePlatform, tt)}</small>
        {unavailable && <small className="wc-dfm-warn" role="status">{unavailable}</small>}
        {actionError && <small className="wc-dfm-error" role="alert">{actionError}</small>}
      </div>
      <div className="opt-ctl fm-ctl">
        {dfmOk && (dfmOk.isDefault
          ? <button className="btn" disabled={busy} onClick={() => void apply(false)}>{busy ? tt("Applying…") : restoreActionLabel(dfmOk, tt)}</button>
          : <button className="btn primary" disabled={busy || !dfmOk.canSet} title={unavailable ?? undefined} onClick={() => void apply(true)}>{busy ? tt("Applying…") : tt("Set Gall as default")}</button>
        )}
        <button className="btn" disabled={busy} onClick={refresh}>{tt("Refresh")}</button>
      </div>
    </div>
    {linux && (
      <div className="opt">
        <div className="opt-text"><span className="opt-label">{tt("Running app for Show in Folder")}</span>
          <small data-file-manager-reveal role="status">{revealText}</small></div>
      </div>
    )}
  </Group>;
}

export function SettingsDialog({ section: initial }: { section?: string }) {
  const s = useStore((st) => st.settings)!;
  const [section, setSection] = useState(initial ?? "appearance");
  const [q, setQ] = useState("");
  const query = q.trim().toLowerCase();
  // Search: matching options grouped as "Section › Group".
  const filtered = useMemo(() => {
    if (!query) return null;
    return SECTIONS.flatMap((sec) => sec.groups.map((g) => ({
      title: `${tt(sec.title)} › ${tt(g.title)}`,
      opts: g.opts.filter((o) => [o.label, o.hint ?? "", g.title, sec.title].some((x) => `${x} ${tt(x)}`.toLowerCase().includes(query))),
    }))).filter((g) => g.opts.length);
  }, [query]);
  const shortcutHits = query ? (Object.keys(DEFAULT_KEYMAP) as Action[]).some((a) => ACTION_LABELS[a].toLowerCase().includes(query) || tt(ACTION_LABELS[a]).toLowerCase().includes(query)) || gitGroups(s.shortcuts, query).length > 0 : false;
  const updateOpt: Opt = {
    key: "autoDownloadUpdates",
    label: "Download updates automatically",
    hint: "Download and verify new releases in the background, then prompt to restart when ready. Turn off to only show a notification.",
    type: "bool",
  };
  const updateHit = query ? (`updates ${updateOpt.label} ${tt(updateOpt.label)} ${updateOpt.hint} ${tt(updateOpt.hint ?? "")}`).toLowerCase().includes(query) : false;
  const cur = SECTIONS.find((x) => x.id === section);
  const extra = EXTRA_NAV.find((x) => x.id === section);
  const go = (id: string) => { setQ(""); setSection(id); };
  const navItem = (x: { id: string; title: string; icon: string }) => (
    <button key={x.id} className={!query && section === x.id ? "on" : ""} onClick={() => go(x.id)}>
      <Icon name={x.icon} size={15} /><span>{tt(x.title)}</span>
    </button>
  );
  return (
    <div className="backdrop center" onMouseDown={() => A.openOverlay(null)}>
      <div className="modal settings" onMouseDown={(e) => e.stopPropagation()} tabIndex={-1} onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Escape") A.openOverlay(null); }}>
        <div className="shead">
          <div className="shead-title"><Icon name="settings" size={17} /><b>{tt("Settings")}</b></div>
          <label className="ssearch">
            <Icon name="search" size={14} />
            <input type="text" autoFocus placeholder={tt("Search settings…")} value={q} onChange={(e) => setQ(e.target.value)} spellCheck={false} />
            {q && <button className="icon-btn" title={tt("Clear")} onClick={() => setQ("")}><Icon name="close" size={12} /></button>}
          </label>
          <button className="btn primary" onClick={() => A.openOverlay(null)}>{tt("Done")}</button>
        </div>
        <div className="sbody">
          <nav>
            {SECTIONS.map(navItem)}
            <div className="nav-sep" />
            {EXTRA_NAV.map(navItem)}
          </nav>
          <div className="panel">
            {filtered ? (
              <>
                <div className="section-head"><h2>{tt("Search")}</h2><p>{tt("{count} settings match “{query}”", { count: filtered.reduce((n, g) => n + g.opts.length, 0) + (updateHit ? 1 : 0) || tt("No"), query: q.trim() })}</p></div>
                {filtered.map((g) => <Group key={g.title} title={g.title}>{g.opts.map((o) => <OptRow key={o.key} o={o} s={s} />)}</Group>)}
                {updateHit && <Group title="Updates"><OptRow o={updateOpt} s={s} /></Group>}
                {shortcutHits && (
                  <section className="sgroup">
                    <h4>{tt("Shortcuts")}</h4>
                    <Shortcuts s={s} q={query} />
                  </section>
                )}
              </>
            ) : section === "about" ? (
              <AboutInfo full />
            ) : (
              <>
                <div className="section-head">
                  <h2>{tt((cur?.title ?? extra?.title)!)}</h2>
                  {(cur?.desc ?? extra?.desc) && <p>{tt((cur?.desc ?? extra?.desc)!)}</p>}
                </div>
                {section === "openWith" && <FileManagerStatus />}
                {section === "updates" ? (
                  <Updates />
                ) : section === "shortcuts" ? (
                  <Shortcuts s={s} q="" />
                ) : (
                  cur?.groups.map((g) => <Group key={g.title} title={g.title}>{g.opts.map((o) => <OptRow key={o.key} o={o} s={s} />)}</Group>)
                )}
                {section === "view" && (
                  <Group title="Forget">
                    <div className="opt">
                      <div className="opt-text"><span className="opt-label">{tt("Folder views")}</span><small>{tt("Every folder goes back to the default sort and view")}</small></div>
                      <div className="opt-ctl"><button className="btn" onClick={() => A.forgetFolderViews()}>{tt("Forget")}</button></div>
                    </div>
                  </Group>
                )}
                {section === "advanced" && <Group title="Local explanation model"><CompareAiSettings /></Group>}
                {section === "advanced" && (
                  <Group title="Factory reset">
                    <div className="opt">
                      <div className="opt-text"><span className="opt-label">{tt("Reset all settings")}</span><small>{tt("Every setting, shortcut, folder colour, remembered program and folder view back to the default. Favourites, colour tags, history and open tabs stay.")}</small></div>
                      <div className="opt-ctl"><button className="btn danger" onClick={() => A.resetAllSettings()}>{tt("Reset…")}</button></div>
                    </div>
                  </Group>
                )}
                {section === "window" && (
                  <Group title="Favourites">
                    <div className="opt">
                      <div className="opt-text"><span className="opt-label">{tt("Default sidebar folders")}</span><small>{tt("Recreate the default group of standard folders (Home, Documents, Downloads…)")}</small></div>
                      <div className="opt-ctl"><button className="btn" onClick={() => A.favCmd({ op: "restoreDefault" })}>{tt("Create")}</button></div>
                    </div>
                    <div className="opt">
                      <div className="opt-text"><span className="opt-label">{tt("Import favorites")}</span><small>{tt("Take over the places of other file managers (GTK bookmarks, KDE Places) as a new group; existing favorites stay as they are")}</small></div>
                      <div className="opt-ctl"><button className="btn" onClick={() => A.openOverlay({ type: "favoritesImport" })}>{tt("Import…")}</button></div>
                    </div>
                  </Group>
                )}
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
