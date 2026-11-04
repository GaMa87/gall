// Settings window (C-01, C-04): searchable, applies changes immediately.
import { cloneElement, isValidElement, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/ipc";
import { FolderLink } from "./FolderLink";
import type { FontFamily, OpenRule, Settings } from "../api/types";
import { ruleLabel } from "../core/openRules";
import { ACTION_LABELS, DEFAULT_KEYMAP, buildKeymap, normalizeChord, type Action } from "../core/keymap";
import * as A from "../state/actions";
import { getState, useStore } from "../state/store";
import { useKeyLabels } from "../state/keyLabels";
import { AboutInfo } from "./AboutInfo";
import { join } from "../core/path";
import { BUILTIN_THEMES } from "../themes";
import { Icon } from "./Icon";
import { VIEWS } from "./FolderPane";

type Opt =
  /** `action`: its keys in the current mode (vim / chords) are shown as the hint. */
  | { key: keyof Settings; label: string; hint?: string; type: "bool"; action?: Action }
  | { key: keyof Settings; label: string; hint?: string; type: "number"; min?: number; max?: number; step?: number }
  | { key: keyof Settings; label: string; hint?: string; type: "text"; placeholder?: string }
  | { key: keyof Settings; label: string; hint?: string; type: "select"; options: [string, string][] }
  | { key: keyof Settings; label: string; hint?: string; type: "theme" }
  | { key: keyof Settings; label: string; hint?: string; type: "openRules" }
  | { key: keyof Settings; label: string; hint?: string; type: "color" }
  | { key: keyof Settings; label: string; hint?: string; type: "list" }
  | { key: keyof Settings; label: string; hint?: string; type: "font" };

type Section = { id: string; title: string; icon: string; desc: string; groups: { title: string; opts: Opt[] }[] };

const SECTIONS: Section[] = [
  {
    id: "appearance", title: "Appearance", icon: "wand", desc: "Colours, theme, fonts and how items look.", groups: [
      { title: "Theme", opts: [
        { key: "theme", label: "Theme", type: "select", options: [["system", "Follow system"], ["dark", "Dark"], ["light", "Light"]] },
        { key: "accent", label: "Accent colour", type: "color" },
        { key: "customTheme", label: "Custom theme", hint: "Included themes and CSS files in <settings>/themes. Ctrl+Shift+F5 reloads the theme while you edit it.", type: "theme" },
        { key: "thinScrollbars", label: "Thin scrollbars", type: "bool" },
        { key: "animations", label: "Animations", type: "bool" },
      ] },
      { title: "Items", opts: [
        { key: "fontFamily", label: "Font", hint: "Any installed font, fixed-width or proportional. Code and text previews keep a fixed-width font.", type: "font" },
        { key: "fontStyle", label: "Font style", type: "select", options: [["regular", "Regular"], ["bold", "Bold"], ["italic", "Italic"], ["boldItalic", "Bold italic"]] },
        { key: "fontSize", label: "Font size", type: "number", min: 9, max: 24, step: 0.5 },
        { key: "rowPadding", label: "Row padding (px)", type: "number", min: 0, max: 12 },
        { key: "showNavColumns", label: "Miller columns above the list", hint: "Default for both browsers in the Standard layout; off shows a breadcrumb bar instead (Columns starts with columns, Single with breadcrumbs). ` or the button in a tab bar switches one browser in any layout, and that choice is remembered per layout.", type: "bool" },
        { key: "showDetailsPane", label: "Show preview & details pane", hint: "Default for both browsers. Alt+P or the pane button in a tab bar shows or hides it in that browser only; Ctrl+U swaps the folders and keeps the panes. Space opens the preview window either way.", type: "bool" },
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
        { key: "showHidden", label: "Show hidden files", type: "bool", action: "toggleHidden" },
        { key: "hideTemp", label: "Hide temporary files", hint: "~, .tmp, .swp, .bak, .part…", type: "bool" },
      ] },
      { title: "Sorting defaults", opts: [
        { key: "foldersOnTop", label: "Keep folders on top", type: "bool" },
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
        { key: "indexExclude", label: "Never index or search", hint: "Comma separated names or globs (gitignore syntax), e.g. node_modules, *.iso", type: "list" },
        { key: "indexHidden", label: "Index hidden files and folders", type: "bool" },
        { key: "indexRespectGitignore", label: "Skip files ignored by .gitignore", type: "bool" },
        { key: "indexRefreshMinutes", label: "Re-index every (minutes)", hint: "0 = only when Quick Open finds the index older than a minute", type: "number", min: 0, max: 1440 },
      ] },
      { title: "Find in files (Ctrl+F / Ctrl+Shift+F)", opts: [
        { key: "searchRespectGitignore", label: "Skip files ignored by .gitignore", type: "bool" },
      ] },
    ],
  },
  {
    id: "keyboard-vim", title: "Vim keys", icon: "code", desc: "Vim-style keys and the leader-key menu.", groups: [
      { title: "Vim-style navigation", opts: [
        { key: "vimKeys", label: "Enable vim keys in the file list", hint: "j/k/h/l, gg/G, Ctrl+D/U, yy/dd/p, / to filter, ? for the full list. Typing to filter then needs /.", type: "bool" },
        { key: "leaderKey", label: "Leader key (vim keys on)", hint: "Opens the which-key menu. Space Space = go to file, as in LazyVim.", type: "select", options: [["Space", "Space (default)"], ["\\", "\\ (backslash, Vim's default)"], [";", "; (semicolon)"], ["Ctrl", "Ctrl once"], ["Ctrl Ctrl", "Ctrl twice"]] },
        { key: "leaderKeyBasic", label: "Leader key (vim keys off)", hint: "Opens the same menu without vim keys. Space and letters keep their usual meaning (preview, type to filter).", type: "select", options: [["Ctrl Ctrl", "Ctrl twice (default)"], ["Ctrl", "Ctrl once"], ["\\", "\\ (backslash)"], [";", "; (semicolon)"], ["", "None"]] },
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
      { title: "Tabs", opts: [
        { key: "newTabPath", label: "New tab opens", hint: "Empty = copy of current tab. Supports ~ and $VARS.", type: "text", placeholder: "(clone current tab)" },
        { key: "newTabPlacement", label: "New tab position", type: "select", options: [["afterCurrent", "After current tab"], ["end", "At the end"]] },
        { key: "tabsShowFullPath", label: "Show full path on tabs", type: "bool" },
        { key: "startupPaths", label: "Start-up folders (first launch / no saved session)", hint: "Separate with ; — the first for browser 1, the second for browser 2", type: "list" },
      ] },
    ],
  },
  {
    id: "window", title: "Window", icon: "window", desc: "Start-up, sidebar and notifications.", groups: [
      { title: "Start-up", opts: [
        { key: "startMaximized", label: "Open maximized", hint: "Applies the next time Gall starts", type: "bool" },
      ] },
      { title: "Sidebar", opts: [
        { key: "showDrives", label: "Show drives", type: "bool" },
        { key: "showCloud", label: "Show cloud folders", hint: "OneDrive, Dropbox, Google Drive, iCloud and others found on this computer", type: "bool" },
        { key: "showHistory", label: "Show history", type: "bool" },
        { key: "showSystemMonitor", label: "Show system monitor", hint: "CPU, GPU, memory, temperatures, network and disk activity at the bottom of the sidebar", type: "bool" },
        { key: "historyCount", label: "Folders remembered in history", type: "number", min: 5, max: 200 },
      ] },
      { title: "Other", opts: [
        { key: "notificationMs", label: "Notification duration (ms)", type: "number", min: 1000, max: 30000, step: 500 },
        { key: "pathCopyFormat", label: "Default path copy format", type: "select", options: [["native", "As is"], ["nativeQuoted", "Quoted"], ["escaped", "Escaped backslashes"], ["forward", "Forward slashes"], ["forwardQuoted", "Forward slashes, quoted"], ["uri", "file:// URI"]] },
      ] },
    ],
  },
  {
    id: "openWith", title: "Open with", icon: "open", desc: "Terminal, editor and the programs remembered for files.", groups: [
      { title: "Terminal", opts: [
        { key: "terminal", label: "Terminal command (F10)", hint: "Easier: Ctrl+F10 (Terminal here with…), pick one, Ctrl+Enter. Empty = auto-detect. {dir} = the folder; quote a program path with spaces.", type: "text", placeholder: "auto" },
      ] },
      { title: "Editor", opts: [
        { key: "editor", label: "Editor command (Edit, F4 / e)", hint: "Easier: Shift+F4 (Edit with…), pick one, Ctrl+Enter. Empty = ask the first time. {path} = the file.", type: "text", placeholder: "ask" },
      ] },
      { title: "Programs for files", opts: [
        { key: "openRules", label: "Remembered programs", hint: "Made with Ctrl+Enter in Open with… (O). The most specific rule wins: one file, then the deepest folder, then everywhere. {path} = the file.", type: "openRules" },
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
  const v = s[o.key] as unknown;
  const set = (val: unknown) => A.updateSettings({ [o.key]: val } as Partial<Settings>);
  const keys = useKeyLabels();
  let ctl: React.ReactNode;
  switch (o.type) {
    case "bool": ctl = <input type="checkbox" className="switch" role="switch" checked={!!v} onChange={(e) => set(e.target.checked)} />; break;
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
    case "text": ctl = <input type="text" value={v as string} placeholder={o.placeholder} onChange={(e) => set(e.target.value)} spellCheck={false} />; break;
    case "select": ctl = <select value={v as string} onChange={(e) => set(e.target.value)}>{o.options.map(([k, l]) => <option key={k} value={k}>{l}</option>)}</select>; break;
    case "theme": ctl = <ThemePicker value={v as string} set={set} />; break;
    case "font": ctl = <FontPicker value={v as string} set={set} />; break;
    case "openRules": return <OpenRulesList rules={v as OpenRule[]} set={set} label={o.label} hint={o.hint} />;
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
  const hint = key ? `Shortcut: ${key}${o.hint ? ` · ${o.hint}` : ""}` : o.hint;
  const id = `opt-${o.key}`;
  return (
    <div className={`opt ${o.type === "bool" ? "toggle" : ""}`}>
      <label htmlFor={id} className="opt-text"><span className="opt-label">{o.label}</span>{hint && <small>{hint}</small>}</label>
      <div className="opt-ctl">{withId(ctl, id)}</div>
    </div>
  );
}

/** Gives the row's control the id its label points at (clicking the label focuses / toggles it). */
function withId(ctl: React.ReactNode, id: string): React.ReactNode {
  return isValidElement(ctl) && (ctl.type === "input" || ctl.type === "select") ? cloneElement(ctl as React.ReactElement<{ id?: string }>, { id }) : ctl;
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
      <select value={value} onFocus={load} onChange={(e) => set(e.target.value)} style={value ? { fontFamily: `"${value}"` } : undefined}>
        <option value="">Default (JetBrains Mono)</option>
        {!known && <option value={value}>{value} {fonts ? "(not installed)" : ""}</option>}
        {fonts == null && <option disabled>Loading fonts…</option>}
        {mono.length > 0 && <optgroup label="Fixed width">{mono.map((f) => <option key={f.name} value={f.name}>{f.name}</option>)}</optgroup>}
        {prop.length > 0 && <optgroup label="Proportional">{prop.map((f) => <option key={f.name} value={f.name}>{f.name}</option>)}</optgroup>}
      </select>
      {cur && (!cur.bold || !cur.italic) && <small className="font-note">no {[!cur.bold && "bold", !cur.italic && "italic"].filter(Boolean).join(" or ")} face: imitated</small>}
    </span>
  );
}

function ThemePicker({ value, set }: { value: string; set: (v: string) => void }) {
  const info = useStore((s) => s.info);
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
  return (
    <span className="theme-pick">
      <select value={value} onFocus={refresh} onMouseDown={refresh} onChange={(e) => set(e.target.value)}>
        <option value="">None (built-in colours)</option>
        <optgroup label="Included">
          {builtins.map(([k, t]) => <option key={k} value={k}>{t.label}{names.includes(k) ? " (your file)" : ""}</option>)}
        </optgroup>
        {files.length > 0 && <optgroup label="Themes folder">{files.map((n) => <option key={n} value={n}>{n}</option>)}</optgroup>}
        {missing && <option value={value}>{value} (file not found)</option>}
      </select>
      <FolderLink path={dir} before={ensureDir} />
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
        <div className="rules-empty">None yet. Select a file, press <kbd>O</kbd> (or Ctrl+Enter), pick a program and press <kbd>Ctrl ↵</kbd>.</div>
      ) : (
        <div className="rules-list">
          {sorted.map((r) => (
            <div key={r.target + (r.dir ?? "")} className="rule">
              <span className="rule-where" title={r.dir ?? r.target}>{ruleLabel(r, home)}</span>
              <span className="rule-name">{r.name}</span>
              <input type="text" value={r.command} spellCheck={false} title="Command; {path} = the file" onChange={(e) => update(r, { command: e.target.value })} />
              <button className="icon-btn" title="Forget (use the system default)" onClick={() => update(r, null)}><Icon name="close" size={12} /></button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function Shortcuts({ s, q }: { s: Settings; q: string }) {
  const actions = (Object.keys(DEFAULT_KEYMAP) as Action[]).filter((a) => !q || ACTION_LABELS[a].toLowerCase().includes(q));
  const keymap = useMemo(() => buildKeymap(s.shortcuts), [s.shortcuts]);
  return (
    <>
      {!q && <p className="section-note">Comma-separate several chords, e.g. <kbd>F5</kbd>, <kbd>Ctrl+R</kbd>. Enter or leaving the field saves; an empty field restores the default.</p>}
      <div className="scard kb-card">
      {actions.map((a) => <ShortcutRow key={a} a={a} s={s} keymap={keymap} />)}
      </div>
    </>
  );
}

/** One action's chords: edited as text and saved on Enter or blur (not on every key, so
 *  clearing the field to type a new chord does not snap back to the default). */
function ShortcutRow({ a, s, keymap }: { a: Action; s: Settings; keymap: Map<string, Action> }) {
  const saved = s.shortcuts[a] ?? DEFAULT_KEYMAP[a].join(", ");
  const [text, setText] = useState(saved);
  useEffect(() => setText(saved), [saved]);
  const commit = (t = text) => {
    const sc = { ...getState().settings!.shortcuts };
    const v = t.trim();
    if (v === "" || v === DEFAULT_KEYMAP[a].join(", ")) delete sc[a];
    else sc[a] = v;
    if (JSON.stringify(sc) !== JSON.stringify(getState().settings!.shortcuts)) A.updateSettings({ shortcuts: sc });
    else setText(saved);
  };
  // Closed by a click outside: no blur comes; save what was typed.
  const latest = useRef({ text, saved, commit });
  latest.current = { text, saved, commit };
  useEffect(() => () => { const l = latest.current; if (l.text !== l.saved) l.commit(l.text); }, []);
  // A chord of this action that another action has: the other one wins or loses it.
  const clash = saved.split(",").map((c) => normalizeChord(c.trim())).filter(Boolean)
    .map((c) => [c, keymap.get(c)] as const).find(([, other]) => other && other !== a);
  return (
    <div className={`kb-row ${s.shortcuts[a] != null ? "changed" : ""}`}>
      <span>{ACTION_LABELS[a]}{clash && <span className="kb-clash" title="Each chord runs one action"> · {clash[0]} runs “{ACTION_LABELS[clash[1]!]}”</span>}</span>
      <input type="text" value={text} spellCheck={false}
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
      <h4>{title}</h4>
      <div className="scard">{children}</div>
    </section>
  );
}

const EXTRA_NAV = [
  { id: "shortcuts", title: "Shortcuts", icon: "list", desc: "Rebind any action. Vim keys and the leader menu are listed in the key sheet." },
  { id: "about", title: "About", icon: "info", desc: "" },
];

export function SettingsDialog({ section: initial }: { section?: string }) {
  const s = useStore((st) => st.settings)!;
  const [section, setSection] = useState(initial ?? "appearance");
  const [q, setQ] = useState("");
  const query = q.trim().toLowerCase();
  // Search: matching options grouped as "Section › Group".
  const filtered = useMemo(() => {
    if (!query) return null;
    return SECTIONS.flatMap((sec) => sec.groups.map((g) => ({
      title: `${sec.title} › ${g.title}`,
      opts: g.opts.filter((o) => (o.label + " " + (o.hint ?? "") + " " + g.title + " " + sec.title).toLowerCase().includes(query)),
    }))).filter((g) => g.opts.length);
  }, [query]);
  const shortcutHits = query ? (Object.keys(DEFAULT_KEYMAP) as Action[]).some((a) => ACTION_LABELS[a].toLowerCase().includes(query)) : false;
  const cur = SECTIONS.find((x) => x.id === section);
  const extra = EXTRA_NAV.find((x) => x.id === section);
  const go = (id: string) => { setQ(""); setSection(id); };
  const navItem = (x: { id: string; title: string; icon: string }) => (
    <button key={x.id} className={!query && section === x.id ? "on" : ""} onClick={() => go(x.id)}>
      <Icon name={x.icon} size={15} /><span>{x.title}</span>
    </button>
  );
  return (
    <div className="backdrop center" onMouseDown={() => A.openOverlay(null)}>
      <div className="modal settings" onMouseDown={(e) => e.stopPropagation()} tabIndex={-1} onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Escape") A.openOverlay(null); }}>
        <div className="shead">
          <div className="shead-title"><Icon name="settings" size={17} /><b>Settings</b></div>
          <label className="ssearch">
            <Icon name="search" size={14} />
            <input type="text" autoFocus placeholder="Search settings…" value={q} onChange={(e) => setQ(e.target.value)} spellCheck={false} />
            {q && <button className="icon-btn" title="Clear" onClick={() => setQ("")}><Icon name="close" size={12} /></button>}
          </label>
          <button className="btn primary" onClick={() => A.openOverlay(null)}>Done</button>
        </div>
        <div className="sbody">
          <nav>
            {SECTIONS.map(navItem)}
            <div className="nav-sep" />
            {EXTRA_NAV.map(navItem)}
          </nav>
          <div className="panel" key={query ? "search" : section}>
            {filtered ? (
              <>
                <div className="section-head"><h2>Search</h2><p>{filtered.reduce((n, g) => n + g.opts.length, 0) || "No"} settings match “{q.trim()}”</p></div>
                {filtered.map((g) => <Group key={g.title} title={g.title}>{g.opts.map((o) => <OptRow key={o.key} o={o} s={s} />)}</Group>)}
                {shortcutHits && (
                  <section className="sgroup">
                    <h4>Shortcuts</h4>
                    <Shortcuts s={s} q={query} />
                  </section>
                )}
              </>
            ) : section === "about" ? (
              <AboutInfo full />
            ) : (
              <>
                <div className="section-head">
                  <h2>{cur?.title ?? extra?.title}</h2>
                  {(cur?.desc ?? extra?.desc) && <p>{cur?.desc ?? extra?.desc}</p>}
                </div>
                {section === "shortcuts" ? (
                  <Shortcuts s={s} q="" />
                ) : (
                  cur?.groups.map((g) => <Group key={g.title} title={g.title}>{g.opts.map((o) => <OptRow key={o.key} o={o} s={s} />)}</Group>)
                )}
                {section === "view" && (
                  <Group title="Forget">
                    <div className="opt">
                      <div className="opt-text"><span className="opt-label">Folder views</span><small>Every folder goes back to the default sort and view</small></div>
                      <div className="opt-ctl"><button className="btn" onClick={() => A.forgetFolderViews()}>Forget</button></div>
                    </div>
                  </Group>
                )}
                {section === "advanced" && (
                  <Group title="Factory reset">
                    <div className="opt">
                      <div className="opt-text"><span className="opt-label">Reset all settings</span><small>Every setting, shortcut, folder colour, remembered program and folder view back to the default. Favourites, colour tags, history and open tabs stay.</small></div>
                      <div className="opt-ctl"><button className="btn danger" onClick={() => A.resetAllSettings()}>Reset…</button></div>
                    </div>
                  </Group>
                )}
                {section === "window" && (
                  <Group title="Favourites">
                    <div className="opt">
                      <div className="opt-text"><span className="opt-label">Default sidebar folders</span><small>Recreate the default group of standard folders (Home, Documents, Downloads…)</small></div>
                      <div className="opt-ctl"><button className="btn" onClick={() => A.favCmd({ op: "restoreDefault" })}>Create</button></div>
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
