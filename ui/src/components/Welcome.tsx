// Welcome setup on the first start (no settings file yet): look, layout, keys and file
// defaults in four short steps. Every choice applies at once, so the window behind shows
// it; "Skip" keeps the defaults. Settings › About › "Welcome setup" opens it again.
import { useEffect, useRef, useState, type ReactNode } from "react";
import type { Layout, Settings } from "../api/types";
import * as A from "../state/actions";
import { useStore } from "../state/store";
import { VIEWS } from "./FolderPane";
import { Icon } from "./Icon";
import { Logo } from "./TitleBar";
import { keyboardProfile } from "../core/keymap";

const ACCENTS = ["#3d8bfd", "#8b5cf6", "#14b8a6", "#22c55e", "#f59e0b", "#ef4444", "#ec4899"];

const STEPS = [
  { title: "Look", heading: "Make it look right", desc: "Pick a theme and an accent colour. Both can be changed later in Settings › Appearance." },
  { title: "Layout", heading: "How many browsers?", desc: "Gall shows two folders at once by default. You can switch any time from the sidebar." },
  { title: "Keyboard", heading: "How do you like your keys?", desc: "Choose familiar shortcuts; every profile includes Gall commands and ? for help. Change it later in Settings › Keyboard." },
  { title: "Files", heading: "Folder defaults", desc: "How a folder is shown and sorted until you change it for that folder." },
] as const;

/** A big selectable card with a picture on top. */
function Choice({ on, onClick, title, sub, children }: { on: boolean; onClick: () => void; title: string; sub?: ReactNode; children?: ReactNode }) {
  return (
    <button className={`wc-choice ${on ? "on" : ""}`} onClick={onClick} aria-pressed={on}>
      {children && <div className="wc-pic">{children}</div>}
      <div className="wc-choice-text">
        <b>{title}{on && <Icon name="check" size={14} />}</b>
        {sub && <span>{sub}</span>}
      </div>
    </button>
  );
}

/** A tiny window in a theme's colours. */
function ThemePic({ mode }: { mode: "dark" | "light" | "system" }) {
  const pane = (dark: boolean) => (
    <div className={`wc-theme ${dark ? "dark" : "light"}`}>
      <i className="wc-t-bar" /><i className="wc-t-line wc-t-acc" /><i className="wc-t-line" /><i className="wc-t-line" /><i className="wc-t-line wc-t-short" />
    </div>
  );
  if (mode === "system") return <div className="wc-theme-split">{pane(false)}{pane(true)}</div>;
  return pane(mode === "dark");
}

/** Browsers as boxes: side by side, stacked, or one. */
function LayoutPic({ layout }: { layout: Layout }) {
  const box = (x: number, y: number, w: number, h: number) => (
    <g key={`${x}-${y}`}>
      <rect x={x} y={y} width={w} height={h} rx="2" className="wc-l-box" />
      <rect x={x} y={y} width={w} height={Math.min(10, h / 3)} rx="2" className="wc-l-head" />
      {[0, 1, 2].map((i) => <rect key={i} x={x + 4} y={y + Math.min(10, h / 3) + 4 + i * 6} width={w * (i === 2 ? 0.4 : 0.7)} height="2.5" rx="1" className="wc-l-line" />)}
    </g>
  );
  return (
    <svg viewBox="0 0 120 72" className="wc-layout">
      <rect x="0" y="0" width="22" height="72" rx="2" className="wc-l-side" />
      {layout === "standard" && [box(26, 0, 45, 72), box(75, 0, 45, 72)]}
      {layout === "columns" && [box(26, 0, 94, 34), box(26, 38, 94, 34)]}
      {layout === "classic" && box(26, 0, 94, 72)}
    </svg>
  );
}

function Toggle({ label, hint, checked, set }: { label: string; hint?: string; checked: boolean; set: (v: boolean) => void }) {
  return (
    <label className="wc-row">
      <span className="wc-row-text"><span>{label}</span>{hint && <small>{hint}</small>}</span>
      <input type="checkbox" className="switch" role="switch" checked={checked} onChange={(e) => set(e.target.checked)} />
    </label>
  );
}

export function Welcome() {
  const s = useStore((st) => st.settings);
  const layout = useStore((st) => st.session?.layout ?? "standard");
  const [step, setStep] = useState(0);
  // Focus once per step (not on every render: choosing an option would pull the focus
  // away from it, and Tab would start over).
  const modalRef = useRef<HTMLDivElement>(null);
  useEffect(() => modalRef.current?.focus(), [step]);
  if (!s) return null;
  const set = (p: Partial<Settings>) => A.updateSettings(p);
  // Saving writes the settings file, so the setup is not offered again (even when skipped).
  const close = () => { A.updateSettings({}); A.openOverlay(null); };
  const last = step === STEPS.length - 1;
  const cur = STEPS[step];

  return (
    <div className="backdrop center welcome-backdrop">
      <div className="modal welcome" tabIndex={-1} ref={modalRef} onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          e.stopPropagation();
          // Enter on a focused button, choice or field is theirs (Back, Skip, a card).
          const own = e.target instanceof HTMLButtonElement || e.target instanceof HTMLSelectElement || e.target instanceof HTMLInputElement;
          if (e.key === "Escape") close();
          else if (e.key === "Enter" && !own) { e.preventDefault(); if (last) close(); else setStep(step + 1); }
        }}>
        <header className="wc-head">
          <Logo size={34} />
          <div>
            <b>Welcome to Gall</b>
            <span>A few choices to start with. Everything here can be changed later in Settings.</span>
          </div>
        </header>

        <ol className="wc-steps">
          {STEPS.map((x, i) => (
            <li key={x.title} className={i === step ? "on" : i < step ? "done" : ""}>
              <button onClick={() => setStep(i)}><span className="wc-num">{i < step ? <Icon name="check" size={11} /> : i + 1}</span>{x.title}</button>
            </li>
          ))}
        </ol>

        <div className="wc-body" key={step}>
          <h2>{cur.heading}</h2>
          <p className="wc-desc">{cur.desc}</p>

          {step === 0 && (
            <>
              <div className="wc-choices three">
                <Choice on={s.theme === "system"} onClick={() => set({ theme: "system" })} title="Follow system" sub="Light or dark with your desktop"><ThemePic mode="system" /></Choice>
                <Choice on={s.theme === "dark"} onClick={() => set({ theme: "dark" })} title="Dark"><ThemePic mode="dark" /></Choice>
                <Choice on={s.theme === "light"} onClick={() => set({ theme: "light" })} title="Light"><ThemePic mode="light" /></Choice>
              </div>
              <div className="wc-accents">
                <span>Accent colour</span>
                {ACCENTS.map((c) => (
                  <button key={c} className={`wc-swatch ${s.accent.toLowerCase() === c ? "on" : ""}`} style={{ background: c }} title={c} onClick={() => set({ accent: c })} />
                ))}
                <label className={`wc-swatch custom ${ACCENTS.includes(s.accent.toLowerCase()) ? "" : "on"}`} title="Custom colour">
                  <Icon name="plus" size={12} />
                  <input type="color" value={s.accent} onChange={(e) => set({ accent: e.target.value })} />
                </label>
              </div>
            </>
          )}

          {step === 1 && (
            <>
              <div className="wc-choices three">
                <Choice on={layout === "standard"} onClick={() => A.setLayout("standard")} title="Side by side" sub="Two browsers, Miller columns on top"><LayoutPic layout="standard" /></Choice>
                <Choice on={layout === "columns"} onClick={() => A.setLayout("columns")} title="Stacked" sub="Two browsers, one above the other"><LayoutPic layout="columns" /></Choice>
                <Choice on={layout === "classic"} onClick={() => A.setLayout("classic")} title="Single" sub="One browser, more room"><LayoutPic layout="classic" /></Choice>
              </div>
              <div className="wc-rows">
                <Toggle label="Miller columns above the list" hint="Side by side layout; off: a breadcrumb bar instead. ` switches it per browser in any layout." checked={s.showNavColumns} set={(v) => set({ showNavColumns: v })} />
                <Toggle label="Preview & details pane" hint="Shows the selected file next to the list; Alt+P toggles it per browser." checked={s.showDetailsPane} set={(v) => set({ showDetailsPane: v })} />
              </div>
            </>
          )}

          {step === 2 && (
            <>
              <div className="wc-choices three">
                <Choice on={keyboardProfile(s) === "explorer"} onClick={() => set({ keyboardProfile: "explorer" })} title="Windows Explorer"
                  sub={<>Type to filter · F2 rename · F3 search · F4 address · F5 refresh · F6 other browser</>}>
                  <div className="wc-keys"><kbd>F2</kbd><kbd>F3</kbd><kbd>F5</kbd></div>
                </Choice>
                <Choice on={keyboardProfile(s) === "totalCommander"} onClick={() => set({ keyboardProfile: "totalCommander" })} title="Total Commander"
                  sub={<>F3 preview · F4 edit · F5 copy · F6 move · F7 folder · F8 trash · type to filter</>}>
                  <div className="wc-keys"><kbd>F5</kbd><kbd>F6</kbd><kbd>F8</kbd></div>
                </Choice>
                <Choice on={keyboardProfile(s) === "vim"} onClick={() => set({ keyboardProfile: "vim" })} title="Vim"
                  sub={<><kbd>j</kbd>/<kbd>k</kbd> move · <kbd>h</kbd>/<kbd>l</kbd> out and in · <kbd>/</kbd> filter · <kbd>Space</kbd> opens a menu of commands · <kbd>?</kbd> all keys</>}>
                  <div className="wc-keys vim"><kbd>h</kbd><kbd>j</kbd><kbd>k</kbd><kbd>l</kbd></div>
                </Choice>
              </div>
              <p className="wc-desc">All profiles keep custom shortcuts and Gall features. Explorer and Commander: tap Ctrl twice for the full command menu. Vim: press Space.</p>
            </>
          )}

          {step === 3 && (
            <div className="wc-rows">
              <div className="wc-row">
                <span className="wc-row-text"><span>View</span><small>How items are listed</small></span>
                <select value={s.defaultView} onChange={(e) => set({ defaultView: e.target.value })}>
                  {VIEWS.map((v) => <option key={v.id} value={v.id}>{v.label}</option>)}
                </select>
              </div>
              <div className="wc-row">
                <span className="wc-row-text"><span>Sort by</span></span>
                <span className="wc-sort">
                  <select value={s.defaultSort} onChange={(e) => set({ defaultSort: e.target.value as Settings["defaultSort"] })}>
                    {[["name", "Name"], ["ext", "Type"], ["modified", "Date modified"], ["created", "Date created"], ["size", "Size"]].map(([k, l]) => <option key={k} value={k}>{l}</option>)}
                  </select>
                  <button className="btn" title="Direction" onClick={() => set({ defaultDescending: !s.defaultDescending })}>
                    {s.defaultDescending ? "Descending ↓" : "Ascending ↑"}
                  </button>
                </span>
              </div>
              <Toggle label="Folders on top" checked={s.foldersOnTop} set={(v) => set({ foldersOnTop: v })} />
              <Toggle label="Remember view and sort per folder" hint="Otherwise each browser keeps one view and sort in every folder" checked={s.rememberFolderViews} set={(v) => set({ rememberFolderViews: v })} />
              <Toggle label="Show hidden files" hint="Dot files and hidden items" checked={s.showHidden} set={(v) => set({ showHidden: v })} />
              <Toggle label="Open with a single click" hint="Otherwise a double click opens, a single click selects" checked={s.singleClickOpen} set={(v) => set({ singleClickOpen: v })} />
            </div>
          )}
        </div>

        <footer className="wc-foot">
          <button className="link-btn wc-skip" onClick={close}>{step === 0 ? "Skip, use the defaults" : "Skip the rest"}</button>
          <span style={{ flex: 1 }} />
          {step > 0 && <button className="btn" onClick={() => setStep(step - 1)}>Back</button>}
          <button className="btn primary" onClick={() => (last ? close() : setStep(step + 1))}>{last ? "Start using Gall" : "Next"}</button>
        </footer>
      </div>
    </div>
  );
}
