// which-key popup for pending vim/leader sequences, and the "?" key sheet.
import { useEffect, useMemo, useState } from "react";
import { ACTION_LABELS, buildKeymap, keyboardProfile, KEYBOARD_PROFILES, type Action } from "../core/keymap";
import type { Mark } from "../api/types";
import { tildify } from "../core/path";
import { LEADER_KEYS, NORMAL_KEYS, PREVIOUS_MARK, resolve, shadowed, splitKeys, type KeyNode, type MarkOp } from "../core/vim";
import * as A from "../state/actions";
import { useStore } from "../state/store";
import { ShortcutKeys, vimSteps, type KeySequence } from "./ShortcutKeys";

const DELAY_MS = 220;

export function WhichKey({ tree }: { tree: KeyNode }) {
  const seq = useStore((s) => s.keySeq);
  const leader = useStore((s) => s.settings?.vimKeys ? s.settings.leaderKey : s.settings?.leaderKeyBasic) ?? "Ctrl";
  const marks = useStore((s) => s.marks.marks);
  const home = useStore((s) => s.info?.home);
  const [shown, setShown] = useState(false);
  useEffect(() => {
    setShown(false);
    if (!seq.length) return;
    const t = setTimeout(() => setShown(true), DELAY_MS);
    return () => clearTimeout(t);
  }, [seq.join(" ")]);

  if (!seq.length) return null;
  const r = resolve(tree, seq);
  if (r.kind !== "pending") return null;
  if (!shown) return <div className="wk-pending"><ShortcutKeys alternatives={[vimSteps(seq, leader)]} /></div>;
  if (r.node.mark) return <MarkList op={r.node.mark} seq={seq} leader={leader} label={r.node.label} marks={marks} home={home} />;
  const items = [...r.node.children!.entries()].sort(([a, x], [b, y]) => {
    const gx = !!x.children, gy = !!y.children;
    if (gx !== gy) return gx ? 1 : -1;
    return a.localeCompare(b);
  });
  return (
    <div className="wk">
      <div className="wk-title">
        <span className="wk-pressed">Pressed</span>
        <ShortcutKeys alternatives={[vimSteps(seq, leader)]} />
        <span>{r.node.label.replace(/^\+/, "") || "keys"}</span>
        <small>Choose the next key · Esc to cancel</small>
      </div>
      <div className="wk-grid">
        {items.map(([k, n]) => (
          <div key={k} className={`wk-item ${n.children ? "group" : ""}`}>
            <ShortcutKeys alternatives={[vimSteps([k], leader)]} />
            <span className="lbl">{n.label.replace(/^\+/, "")}</span>
            {n.children && <span className="wk-more">more</span>}
          </div>
        ))}
      </div>
      <div className="wk-hint">Release each step before pressing the next key.</div>
    </div>
  );
}

/** Mark keys in the order a list shows them: the previous folder, then a–z, then A–Z. */
function markOrder(a: string, b: string): number {
  const rank = (k: string) => (k === PREVIOUS_MARK ? 0 : k === k.toLowerCase() ? 1 : 2);
  return rank(a) - rank(b) || a.localeCompare(b);
}

/** which-key for m / ' / ` / dm: the marks that exist, each with its folder. */
function MarkList({ op, seq, leader, label, marks, home }: {
  op: MarkOp; seq: string[]; leader: string; label: string; marks: Record<string, Mark>; home?: string;
}) {
  // ' is reached as '' / ``; it cannot be set or deleted by hand.
  const keys = Object.keys(marks).filter((k) => op === "jump" || op === "jumpItem" || k !== PREVIOUS_MARK).sort(markOrder);
  const hint = op === "set" ? "Type a letter (a–z, A–Z) · a used one is replaced" : "Type the mark's letter";
  return (
    <div className="wk wk-marks">
      <div className="wk-title">
        <span className="wk-pressed">Pressed</span>
        <ShortcutKeys alternatives={[vimSteps(seq, leader)]} />
        <span>{label}</span>
        <small>{hint} · Esc to cancel</small>
      </div>
      {keys.length ? (
        <div className="wk-grid">
          {keys.map((k) => (
            <div key={k} className="wk-item" title={marks[k].path}>
              <ShortcutKeys alternatives={[vimSteps(k === PREVIOUS_MARK ? [seq[seq.length - 1]] : [k], leader)]} />
              <span className="lbl">
                {k === PREVIOUS_MARK && <span className="wk-mark-prev">before last jump · </span>}
                {tildify(marks[k].path, home)}
              </span>
            </div>
          ))}
        </div>
      ) : (
        <div className="wk-empty">{op === "set" ? "No marks yet." : "No marks yet: m and a letter marks the current folder."}</div>
      )}
      <div className="wk-hint">Marks are kept between sessions.</div>
    </div>
  );
}

type HelpRow = { bindings?: readonly KeySequence[]; text?: string; label: string };
const either = (...chords: string[]): KeySequence[] => chords.map((chord) => [chord]);

function specRows(specs: typeof NORMAL_KEYS, prefix: string[] = []): HelpRow[] {
  const rows = new Map<Action, HelpRow>();
  for (const [spec, label, action] of specs) {
    if (!action) continue;
    const binding = [...prefix, ...vimSteps(splitKeys(spec))];
    const previous = rows.get(action);
    rows.set(action, { label: previous?.label ?? label, bindings: [...(previous?.bindings ?? []), binding] });
  }
  return [...rows.values()];
}

/** Full keyboard reference with explicit chords, sequences and alternatives. */
export function KeyHelp() {
  const settings = useStore((s) => s.settings)!;
  const profile = keyboardProfile(settings);
  const leader = settings.vimKeys ? settings.leaderKey || "Space" : settings.leaderKeyBasic ?? "Ctrl Ctrl";
  const [q, setQ] = useState("");
  const chords = useMemo(() => {
    const km = buildKeymap(settings.shortcuts, profile);
    const by = new Map<Action, KeySequence[]>();
    for (const [c, a] of km) if (!shadowed(c, { vim: settings.vimKeys, keymap: km, leader })) by.set(a, [...(by.get(a) ?? []), [c]]);
    return [...by.entries()].map(([a, bindings]) => ({ bindings, label: ACTION_LABELS[a] }));
  }, [settings.shortcuts, settings.vimKeys, profile, leader]);

  const section = (title: string, rows: HelpRow[]) => {
    const vis = rows.filter((r) => !q || [r.label, r.text, ...(r.bindings?.map((steps) => steps.join(" ")) ?? [])].join(" ").toLowerCase().includes(q.toLowerCase()));
    if (!vis.length) return null;
    return (
      <section key={title}>
        <h4>{title}</h4>
        {vis.map((r) => (
          <div key={r.label + JSON.stringify(r.bindings ?? r.text)} className="kh-row">
            <span className="kh-action">{r.label}</span>
            {r.bindings ? <ShortcutKeys alternatives={r.bindings} /> : <code className="kh-literal">{r.text}</code>}
          </div>
        ))}
      </section>
    );
  };
  const hasLeader = !!leader;
  const vimRows = specRows(NORMAL_KEYS);
  const leaderSteps = leader === "Ctrl Ctrl" ? ["Ctrl", "Ctrl"] : vimSteps([leader]);
  const leaderRows = specRows(LEADER_KEYS, leaderSteps);

  const sections = [
    settings.vimKeys && section("File list (vim)", vimRows),
    settings.vimKeys && section("Folder marks (vim)", [
      { text: "m a…z / A…Z", label: "mark this folder (kept between sessions)" },
      { text: "' a…z / A…Z", label: "go to the marked folder" },
      { text: "` a…z / A…Z", label: "go there and select the item focused when marked" },
      { text: "'' or ``", label: "back to the folder before the last jump" },
      { text: "d m a…z / A…Z", label: "delete a mark" },
    ]),
    section("File list (standard)", [
      { bindings: either("ArrowUp", "ArrowDown"), label: "move between items" },
      { bindings: either("ArrowLeft", "ArrowRight"), label: "parent / enter folder (grid: move between items)" },
      { bindings: either("Home", "End"), label: "first / last item" },
      { bindings: either("PageUp", "PageDown"), label: "page up / down" },
      { bindings: either("Shift+ArrowUp", "Shift+ArrowDown"), label: "extend selection (also Home/End, PgUp/PgDn)" },
      { bindings: either("Ctrl+ArrowUp", "Ctrl+ArrowDown"), label: "move cursor without changing selection" },
      { bindings: either("Escape"), label: "clear filter / selection" },
      ...(!settings.vimKeys ? [{ text: "type a name", label: "filter this folder (Backspace erases, Esc clears)" }] : []),
    ]),
    hasLeader && section("Leader sequences", leaderRows),
    section("Shortcuts", chords),
    section("Quick Open query syntax", [
      { text: "abc", label: "fuzzy: letters in order" },
      { text: "'abc", label: "exact substring" },
      { text: "^abc", label: "name starts with" },
      { text: "abc$", label: "name ends with" },
      { text: "!abc", label: "exclude" },
      { text: "*.pdf", label: "wildcard on name" },
      { text: "ext:rs,toml", label: "extension is one of" },
      { text: "src/x", label: "match against full path" },
      { text: "abc/", label: "directories only (ends with /)" },
      { bindings: either("Tab"), label: "toggle everywhere / this folder" },
      { text: "(empty query)", label: "recent files and folders" },
      { bindings: either("Enter"), label: "open" },
      { bindings: either("Ctrl+Enter", "Alt+Enter"), label: "select it in its folder" },
      { bindings: either("Shift+Enter"), label: "open it in the other browser" },
      { bindings: either("Alt+I"), label: "name search here, including ignored files" },
      { text: ">", label: "command palette" },
    ]),
    section("Find in files", [
      { bindings: either("Tab", "Shift+Tab"), label: "scope: folder → Home → whole drive (Shift goes back)" },
      { bindings: [["ArrowDown"], ["F4"], ["Ctrl+j"], ["Ctrl+n"]], label: "next match" },
      { bindings: [["ArrowUp"], ["Shift+F4"], ["Ctrl+k"], ["Ctrl+p"]], label: "previous match" },
      { bindings: either("Enter"), label: "open the file" },
      { bindings: either("Ctrl+Enter", "Shift+Enter"), label: "select it in its folder (Shift: in the other browser)" },
      { bindings: either("Alt+C"), label: "match case" },
      { bindings: either("Alt+W"), label: "whole word" },
      { bindings: either("Alt+R"), label: "regular expression" },
      { bindings: either("Alt+I"), label: "also search ignored files (also in name search)" },
      { bindings: either("Escape"), label: "stop the search, then close" },
    ]),
    section("Git workspace", [
      settings.vimKeys
        ? { bindings: either("?"), label: "inside the workspace: keys of the focused panel" }
        : { text: "? button in its header", label: "list the workspace's actions (keys need Vim)" },
      { text: "click the Git status", label: "open it from the status bar" },
    ]),
    section("Preview window", [
      { bindings: either("ArrowUp", "ArrowDown"), label: "previous / next file" },
      ...(settings.vimKeys ? [
        { bindings: either("j", "k"), label: "scroll down / up" },
        { bindings: either("Ctrl+d", "Ctrl+u"), label: "half page down / up" },
        { bindings: either("Ctrl+f", "Ctrl+b"), label: "page down / up" },
        { bindings: [["g", "g"], ["Shift+g"]], label: "top / bottom" },
      ] : []),
      { bindings: either("PageDown", "PageUp"), label: "page down / up" },
      { bindings: either("Home", "End"), label: "top / bottom" },
      { bindings: either("ArrowLeft", "ArrowRight"), label: "PDF: previous / next page · video: seek" },
      { bindings: either("+", "-"), label: "image / PDF: zoom in / out (wheel zooms at the pointer)" },
      { bindings: either("0", "1"), label: "image: fit / actual size (double-click fits)" },
      { bindings: either("[", "]"), label: "video: previous / next frame" },
      { bindings: either("k"), label: "video: play / pause" },
      { bindings: either("Escape", "Space"), label: "close" },
    ]),
    section("Open with", [
      { text: "1 … 9", label: "pick that program (with an empty filter)" },
      { bindings: either("Ctrl+Enter", "Ctrl+1"), label: "open and remember the program" },
      { bindings: either("Tab", "Shift+Tab"), label: "remember for: type, project, folder or this file" },
      { text: "type a command", label: "run your own program" },
    ]),
    section("Compare files", [
      { bindings: [["n"], ["]"], ["F8"]], label: "next change (text, table, hex)" },
      { bindings: [["Shift+n"], ["["], ["Shift+F8"]], label: "previous change" },
      { bindings: either("u"), label: "text: split / unified" },
      { bindings: either("Alt+ArrowRight", "Alt+ArrowLeft"), label: "text: copy the change to the right / left" },
      { bindings: either("Ctrl+S", "Ctrl+Z"), label: "text: save edits / undo an edit" },
      { bindings: [["w"], ["i"], ["e"], ["m"]], label: "text: ignore whitespace / case / line ends · minimal diff" },
      { bindings: [["z"], ["l"]], label: "text: fold unchanged · wrap lines" },
      { bindings: [["s"], ["r"], ["y"]], label: "text: swap sides · reload · copy patch" },
      { bindings: either("c"), label: "spreadsheet: only changes" },
      { text: "1 … 4", label: "image: side by side, swipe, onion skin, difference" },
      { bindings: either("[", "]"), label: "image: move the divider / opacity" },
      { bindings: either("Escape", "q"), label: "close" },
    ]),
    section("Compare folders", [
      { bindings: [["n"], ["]"]], label: "next difference (Shift+n or [ goes back)" },
      { bindings: either(">", "<"), label: "copy the selected items to the right / left" },
      { text: "Space (in that window)", label: "select item" },
      { bindings: [["c"], ["."], ["a"]], label: "compare contents · hidden items · all / differences only" },
      { bindings: [["s"], ["r"], ["/"]], label: "swap sides · rescan · search" },
      { bindings: either("Enter"), label: "open item, or run the planned sync" },
    ]),
    section("Dialogs", [
      { bindings: either("Enter"), label: "default button (or the focused one)" },
      { bindings: either("Escape", ...(settings.vimKeys ? ["q"] : [])), label: "cancel" },
      { bindings: either("ArrowLeft", "ArrowRight", ...(settings.vimKeys ? ["h", "l"] : [])), label: "move between buttons" },
      { text: "underlined letter", label: "activate the matching button" },
      { bindings: either("F2"), label: "rename: select name → extension → all" },
      { bindings: either("Ctrl+Enter"), label: "new folder: create and open it" },
      { bindings: either("Shift+Enter"), label: "new folder: create and move the selection inside" },
      { bindings: either("q"), label: "close any window except dialogs and notes (every profile)" },
      { text: "hold Ctrl in a menu", label: "show the alternative item (Edit with…, Terminal with…, extract and trash)" },
      { bindings: either("c"), label: "item exists: cancel" },
      { bindings: either("s"), label: "item exists: skip" },
      { bindings: either("r"), label: "item exists: replace" },
      { bindings: either("k"), label: "item exists: keep both" },
      { bindings: either("m"), label: "item exists: merge" },
    ]),
    section("Mouse (tabs)", [
      { bindings: either("Middle-click"), label: "close tab (the last tab stays open)" },
      { bindings: either("Alt+Middle-click"), label: "move tab to other browser" },
      { bindings: either("Wheel"), label: "previous / next tab" },
    ]),
    section("Mouse (folders and lists)", [
      { bindings: either("Middle-click"), label: "folder in columns, breadcrumbs or sidebar: open in a new tab" },
      { bindings: either("Alt+Middle-click"), label: "folder in Miller columns: open in the other browser" },
      { bindings: either("Ctrl+Wheel"), label: "grid view: thumbnail size" },
      { text: "drag on empty space or middle-drag", label: "select with a rectangle" },
      { text: "double-click the divider", label: "centre it" },
      { text: "right-click ‹ ⇄ › in the title bar", label: "hide a browser, swap folders, details panes" },
    ]),
    section("Mouse (drag & drop)", [
      { bindings: either("Ctrl+Drag"), label: "copy" },
      { bindings: either("Shift+Drag"), label: "move" },
      { bindings: either("Alt+Drag"), label: "create symbolic links" },
      { text: "drop a file on a file", label: "compare the two files" },
    ]),
    section("Bug report clip", [
      { bindings: either("Ctrl+Shift+Enter"), label: "stop recording" },
      { bindings: either("Ctrl+Shift+Backspace"), label: "cancel recording" },
    ]),
  ].filter(Boolean);

  return (
    <div className="backdrop glass center" onMouseDown={() => A.openOverlay(null)}>
      <div className="kh" onMouseDown={(e) => e.stopPropagation()} tabIndex={-1}
        onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Escape") A.openOverlay(null); }}>
        <div className="kh-head">
          <h3>Keyboard</h3>
          <input autoFocus value={q} placeholder="Filter shortcuts…" onChange={(e) => setQ(e.target.value)} />
          <span className="pill on">{KEYBOARD_PROFILES[profile]}</span>
        </div>
        <div className="kh-legend" aria-label="How to read shortcuts">
          <div><span className="kh-legend-label">Together</span><ShortcutKeys alternatives={[["Ctrl+C"]]} /><small>Hold at the same time</small></div>
          <div><span className="kh-legend-label">In order</span><ShortcutKeys alternatives={[["g", "g"]]} /><small>Release between steps</small></div>
          <div><span className="kh-legend-label">Either one</span><ShortcutKeys alternatives={either("Ctrl+C", "Ctrl+Insert")} /><small>Choose one shortcut</small></div>
          <div><span className="kh-legend-label">Letter shortcuts</span><small>Caps Lock is ignored; hold Shift for the shifted-letter shortcut.</small></div>
        </div>
        <div className="kh-scroll">
          {sections.length ? <div className="kh-cols">{sections}</div>
            : <div className="kh-empty" role="status"><b>No shortcuts match “{q}”</b><span>Try an action name or a key such as Ctrl+C.</span></div>}
        </div>
      </div>
    </div>
  );
}
