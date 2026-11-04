// which-key popup for pending vim/leader sequences, and the "?" key sheet.
import { useEffect, useMemo, useState } from "react";
import { ACTION_LABELS, buildKeymap, type Action } from "../core/keymap";
import { LEADER_KEYS, NORMAL_KEYS, resolve, shadowedByVim, showKey, showKeys, splitKeys, type KeyNode } from "../core/vim";
import * as A from "../state/actions";
import { useStore } from "../state/store";

const DELAY_MS = 220;

export function WhichKey({ tree }: { tree: KeyNode }) {
  const seq = useStore((s) => s.keySeq);
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
  if (!shown) return <div className="wk-pending">{showKeys(seq)}</div>;
  const items = [...r.node.children!.entries()].sort(([a, x], [b, y]) => {
    const gx = !!x.children, gy = !!y.children;
    if (gx !== gy) return gx ? 1 : -1;
    return a.localeCompare(b);
  });
  return (
    <div className="wk">
      <div className="wk-title">
        <kbd>{showKeys(seq)}</kbd>
        <span>{r.node.label.replace(/^\+/, "") || "keys"}</span>
        <small>Esc to cancel</small>
      </div>
      <div className="wk-grid">
        {items.map(([k, n]) => (
          <div key={k} className={`wk-item ${n.children ? "group" : ""}`}>
            <kbd>{showKey(k)}</kbd>
            <span className="arrow">→</span>
            <span className="lbl">{n.label}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

/** Full keyboard reference: vim keys, leader menu and chord shortcuts. */
export function KeyHelp() {
  const settings = useStore((s) => s.settings)!;
  const leader = settings.vimKeys ? settings.leaderKey || "Space" : settings.leaderKeyBasic ?? "Ctrl Ctrl";
  const [q, setQ] = useState("");
  const chords = useMemo(() => {
    const km = buildKeymap(settings.shortcuts);
    const by = new Map<Action, string[]>();
    // With vim keys on, Ctrl+h/j/k/l/d/u/… are vim's: don't list the chords they shadow.
    for (const [c, a] of km) if (!(settings.vimKeys && shadowedByVim(c))) by.set(a, [...(by.get(a) ?? []), c]);
    return [...by.entries()].map(([a, cs]) => ({ keys: cs.join("  "), label: ACTION_LABELS[a] }));
  }, [settings.shortcuts, settings.vimKeys]);

  const f = (label: string, keys: string) => !q || (label + " " + keys).toLowerCase().includes(q.toLowerCase());
  const section = (title: string, rows: { keys: string; label: string; group?: boolean }[]) => {
    const vis = rows.filter((r) => f(r.label, r.keys));
    if (!vis.length) return null;
    return (
      <section>
        <h4>{title}</h4>
        {vis.map((r) => (
          <div key={title + r.keys} className={`kh-row ${r.group ? "group" : ""}`}>
            <kbd>{r.keys}</kbd>
            <span>{r.label}</span>
          </div>
        ))}
      </section>
    );
  };
  const lead = leader === "Space" ? "␣" : leader;
  const hasLeader = !!leader;
  const vimRows = NORMAL_KEYS.map(([k, label, a]) => ({ keys: splitKeys(k).join(" "), label, group: !a }));
  const leaderRows = LEADER_KEYS.map(([k, label, a]) => ({ keys: [lead, ...splitKeys(k).map((t) => (t === "Space" ? "␣" : t))].join(" "), label, group: !a }));

  return (
    <div className="backdrop glass center" onMouseDown={() => A.openOverlay(null)}>
      <div className="kh" onMouseDown={(e) => e.stopPropagation()} tabIndex={-1}
        onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Escape") A.openOverlay(null); }}>
        <div className="kh-head">
          <h3>Keyboard</h3>
          <input autoFocus value={q} placeholder="Filter shortcuts…" onChange={(e) => setQ(e.target.value)} />
          <span className={`pill ${settings.vimKeys ? "on" : ""}`}>vim keys {settings.vimKeys ? "on" : "off"}</span>
        </div>
        {/* The scroll box holds the columns: a multi-column box of fixed height would put
            what does not fit into more columns to the right, reachable only sideways. */}
        <div className="kh-scroll">
        <div className="kh-cols">
          {settings.vimKeys && section("File list (vim)", vimRows)}
          {hasLeader && section(`Leader (${lead})`, leaderRows)}
          {section("Shortcuts", chords)}
          {section("Quick Open query syntax", [
            { keys: "abc", label: "fuzzy: letters in order" },
            { keys: "'abc", label: "exact substring" },
            { keys: "^abc", label: "name starts with" },
            { keys: "abc$", label: "name ends with" },
            { keys: "!abc", label: "exclude" },
            { keys: "*.pdf", label: "wildcard on name" },
            { keys: "ext:rs,toml", label: "extension is one of" },
            { keys: "src/x", label: "match against full path" },
            { keys: "abc/", label: "directories only (ends with /)" },
            { keys: "Tab", label: "toggle everywhere / this folder" },
            { keys: ">", label: "command palette" },
          ])}
          {section("Preview (i)", [
            { keys: "↑ ↓", label: "previous / next file" },
            ...(settings.vimKeys ? [
              { keys: "j k", label: "scroll down / up" },
              { keys: "Ctrl+d Ctrl+u", label: "half page down / up" },
              { keys: "Ctrl+f Ctrl+b", label: "page down / up" },
              { keys: "gg G", label: "top / bottom" },
            ] : []),
            { keys: "PgDn PgUp", label: "page down / up" },
            { keys: "Home End", label: "top / bottom" },
            { keys: "← →", label: "PDF: previous / next page" },
            { keys: "Esc Space", label: "close" },
          ])}
          {section("Dialogs", [
            { keys: "Enter", label: "default button (or the focused one)" },
            { keys: "Esc", label: settings.vimKeys ? "cancel (also q)" : "cancel" },
            { keys: settings.vimKeys ? "← → / h l" : "← →", label: "move between buttons" },
            { keys: "letter", label: "the button with that letter underlined" },
            { keys: "C S R K M", label: "item exists: cancel, skip, replace, keep both, merge" },
          ])}
        </div>
        </div>
      </div>
    </div>
  );
}
