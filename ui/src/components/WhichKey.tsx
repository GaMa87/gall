// which-key popup for pending vim/leader sequences, and the "?" key sheet.
import { useEffect, useMemo, useState } from "react";
import { ACTION_LABELS, buildKeymap, keyboardProfile, KEYBOARD_PROFILES, type Action } from "../core/keymap";
import { LEADER_KEYS, NORMAL_KEYS, resolve, shadowed, splitKeys, type KeyNode } from "../core/vim";
import * as A from "../state/actions";
import { useStore } from "../state/store";
import { ShortcutKeys, vimSteps, type KeySequence } from "./ShortcutKeys";

const DELAY_MS = 220;

export function WhichKey({ tree }: { tree: KeyNode }) {
  const seq = useStore((s) => s.keySeq);
  const leader = useStore((s) => s.settings?.vimKeys ? s.settings.leaderKey : s.settings?.leaderKeyBasic) ?? "Ctrl";
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
      { text: ">", label: "command palette" },
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
      { bindings: either("ArrowLeft", "ArrowRight"), label: "PDF: previous / next page" },
      { bindings: either("Escape", "Space"), label: "close" },
    ]),
    section("Dialogs", [
      { bindings: either("Enter"), label: "default button (or the focused one)" },
      { bindings: either("Escape", ...(settings.vimKeys ? ["q"] : [])), label: "cancel" },
      { bindings: either("ArrowLeft", "ArrowRight", ...(settings.vimKeys ? ["h", "l"] : [])), label: "move between buttons" },
      { text: "underlined letter", label: "activate the matching button" },
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
