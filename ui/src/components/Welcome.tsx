import { ThemedSelect } from "./ThemedSelect";
import { FavoritesImportPanel } from "./FavoritesImport";
import { api } from "../api/ipc";
import type { ImportSourceInfo } from "../core/favoritesImport";
import { fallbackSystemManager, keepOrRestoreTitle, platformScopeNote, unavailableExplanation } from "../core/defaultFileManager";
import { tt } from "../i18n";
// Welcome setup on the first start (no settings file yet): look, layout, keys, file
// defaults and default file manager integration. Every choice applies at once, so the
// window behind shows it; "Skip" keeps the defaults. Settings › About › "Welcome setup"
// opens it again.
import { useEffect, useRef, useState, type ReactNode } from "react";
import type { DefaultFileManagerStatus, Layout, Settings } from "../api/types";
import * as A from "../state/actions";
import { useStore } from "../state/store";
import { GROUP_LABEL, VIEWS } from "./FolderPane";
import { GROUP_MODES } from "../core/sortGroup";
import { Icon } from "./Icon";
import { Logo } from "./TitleBar";
import { keyboardProfile } from "../core/keymap";
import { platformLabel } from "../core/keymap";

const ACCENTS = ["#3d8bfd", "#8b5cf6", "#14b8a6", "#22c55e", "#f59e0b", "#ef4444", "#ec4899"];

type StepId = "look" | "layout" | "keyboard" | "files" | "defaultFm" | "favorites";
interface StepDef { id: StepId; title: string; heading: string; desc: string }

const STEPS: readonly StepDef[] = [
  { id: "look", title: "Look", heading: "Make it look right", desc: "Pick a theme and an accent colour. Both can be changed later in Settings › Appearance." },
  { id: "layout", title: "Layout", heading: "How many browsers?", desc: "Gall shows two folders at once by default. You can switch any time with the layout buttons in the title bar." },
  { id: "keyboard", title: "Keyboard", heading: "How do you like your keys?", desc: "Choose familiar shortcuts; every profile includes Gall commands and ? for help. Change it later in Settings › Keyboard." },
  { id: "files", title: "Files", heading: "Folder defaults", desc: "How a folder is shown and sorted until you change it for that folder." },
  { id: "defaultFm", title: "Default", heading: "Use Gall as your default file manager?", desc: "Open folders from other apps in Gall instead of your system file manager. You can change or restore this any time in Settings › Open with." },
];
/** Offered as the last step, and only when another file manager's favorites were found. */
const FAVORITES_STEP: StepDef = { id: "favorites", title: "Favorites", heading: "Bring your places along", desc: "Gall found the favorites of another file manager. Tick the places you want in the sidebar; they are added as a new group, and the other manager is not touched." };

/** A big selectable card with a picture on top. */
function Choice({ on, disabled, onClick, title, sub, children }: { on: boolean; disabled?: boolean; onClick: () => void; title: string; sub?: ReactNode; children?: ReactNode }) {
  return (
    <button type="button" className={`wc-choice ${on ? "on" : ""}`} disabled={disabled} onClick={onClick} aria-pressed={on}>
      {children && <div className="wc-pic">{children}</div>}
      <div className="wc-choice-text">
        <b>{title}{on && <Icon name="check" size={14} />}</b>
        {sub && <span>{sub}</span>}
      </div>
    </button>
  );
}

/** Folder-handler illustration for the Default file manager step. */
function DefaultFmPic({ gall }: { gall: boolean }) {
  return (
    <svg viewBox="0 0 120 72" className="wc-layout" aria-hidden="true">
      <path d="M18 18h26l6 7h52a5 5 0 0 1 5 5v28a5 5 0 0 1-5 5H18a5 5 0 0 1-5-5V23a5 5 0 0 1 5-5Z" className="wc-l-box" />
      <rect x="21" y="31" width="78" height="24" rx="3" className={gall ? "wc-l-head" : "wc-l-side"} />
      {gall ? (
        <>
          <rect x="27" y="37" width="30" height="12" rx="2" className="wc-l-box" />
          <rect x="63" y="37" width="30" height="12" rx="2" className="wc-l-box" />
        </>
      ) : (
        <>
          <rect x="27" y="38" width="54" height="3" rx="1.5" className="wc-l-line" />
          <rect x="27" y="45" width="36" height="3" rx="1.5" className="wc-l-line" />
        </>
      )}
    </svg>
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
      <input type="checkbox" className="switch" role="switch" title={label} checked={checked} onChange={(e) => set(e.target.checked)} />
    </label>
  );
}

export function Welcome() {
  const s = useStore((st) => st.settings);
  const platform = useStore((st) => st.info?.platform ?? "linux");
  const layout = useStore((st) => st.session?.layout ?? "standard");
  const [step, setStep] = useState(0);
  // Detection only runs when the Welcome setup opens (never at startup).
  const [sources, setSources] = useState<ImportSourceInfo[]>([]);
  const [dfm, setDfm] = useState<DefaultFileManagerStatus | null>(null);
  const [dfmBusy, setDfmBusy] = useState(false);
  const [dfmError, setDfmError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    api.favoritesImportDetect().then((found) => { if (alive) setSources(found); }).catch(() => {});
    api.defaultFileManagerStatus().then((st) => { if (alive) setDfm(st); }).catch(() => {});
    return () => { alive = false; };
  }, []);
  const steps: readonly StepDef[] = sources.length ? [...STEPS, FAVORITES_STEP] : STEPS;
  // Focus once per step (not on every render: choosing an option would pull the focus
  // away from it, and Tab would start over).
  const modalRef = useRef<HTMLDivElement>(null);
  useEffect(() => modalRef.current?.focus(), [step]);
  if (!s) return null;
  const set = (p: Partial<Settings>) => A.updateSettings(p);
  // Saving writes the settings file, so the setup is not offered again (even when skipped).
  const close = () => { A.updateSettings({}); A.openOverlay(null); };
  const last = step === steps.length - 1;
  const cur = steps[step];
  const activePlatform = dfm?.platform ?? platform;
  const unavailable = dfm && !dfm.canSet && !dfm.isDefault ? unavailableExplanation(dfm.unavailableReason, tt) : null;

  const applyDefaultFm = async (enable: boolean) => {
    if (dfmBusy) return;
    if (enable && dfm?.isDefault) return;
    if (!enable && !dfm?.isDefault) return;
    setDfmBusy(true);
    setDfmError(null);
    try {
      const next = enable ? await api.defaultFileManagerSet() : await api.defaultFileManagerRestore();
      setDfm(next);
    } catch (err) {
      setDfmError(String(err instanceof Error ? err.message : err));
    } finally {
      setDfmBusy(false);
    }
  };

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
            <b>{tt("Welcome to Gall")}</b>
            <span>{tt("A few choices to start with. Everything here can be changed later in Settings.")}</span>
          </div>
        </header>

        <ol className="wc-steps">
          {steps.map((x, i) => (
            <li key={x.id} className={i === step ? "on" : i < step ? "done" : ""}>
              <button onClick={() => setStep(i)}><span className="wc-num">{i < step ? <Icon name="check" size={11} /> : i + 1}</span>{tt(x.title)}</button>
            </li>
          ))}
        </ol>

        <div className="wc-body" key={cur.id}>
          <h2>{tt(cur.heading)}</h2>
          <p className="wc-desc">{tt(cur.desc)}</p>

          {cur.id === "look" && (
            <>
              <div className="wc-choices three">
                <Choice on={s.theme === "system"} onClick={() => set({ theme: "system" })} title={tt("Follow system")} sub="Light or dark with your desktop"><ThemePic mode="system" /></Choice>
                <Choice on={s.theme === "dark"} onClick={() => set({ theme: "dark" })} title={tt("Dark")}><ThemePic mode="dark" /></Choice>
                <Choice on={s.theme === "light"} onClick={() => set({ theme: "light" })} title={tt("Light")}><ThemePic mode="light" /></Choice>
              </div>
              <div className="wc-accents">
                <span>{tt("Accent colour")}</span>
                {ACCENTS.map((c) => (
                  <button key={c} className={`wc-swatch ${s.accent.toLowerCase() === c ? "on" : ""}`} style={{ background: c }} title={c} onClick={() => set({ accent: c })} />
                ))}
                <label className={`wc-swatch custom ${ACCENTS.includes(s.accent.toLowerCase()) ? "" : "on"}`} title={tt("Custom colour")}>
                  <Icon name="plus" size={12} />
                  <input type="color" value={s.accent} onChange={(e) => set({ accent: e.target.value })} />
                </label>
              </div>
            </>
          )}

          {cur.id === "layout" && (
            <>
              <div className="wc-choices three">
                <Choice on={layout === "standard"} onClick={() => A.setLayout("standard")} title={tt("Side by side")} sub="Two browsers, Miller columns on top"><LayoutPic layout="standard" /></Choice>
                <Choice on={layout === "columns"} onClick={() => A.setLayout("columns")} title={tt("Stacked")} sub="Two browsers, one above the other"><LayoutPic layout="columns" /></Choice>
                <Choice on={layout === "classic"} onClick={() => A.setLayout("classic")} title={tt("Single")} sub="One browser, more room"><LayoutPic layout="classic" /></Choice>
              </div>
              <div className="wc-rows">
                <Toggle label={tt("Miller columns above the list")} hint="Side by side layout; off: a breadcrumb bar instead. The button in a tab bar switches it per browser in any layout." checked={s.showNavColumns} set={(v) => set({ showNavColumns: v })} />
                <Toggle label={tt("Preview & details pane")} hint={platformLabel("Shows the selected file next to the list; Alt+P toggles it per browser.")} checked={s.showDetailsPane} set={(v) => set({ showDetailsPane: v })} />
              </div>
            </>
          )}

          {cur.id === "keyboard" && (
            <>
              <div className="wc-choices three">
                <Choice on={keyboardProfile(s) === "explorer"} onClick={() => set({ keyboardProfile: "explorer" })} title={tt("Windows Explorer")}
                  sub={<>{tt("Type to filter · F2 rename · F3 search · F4 address · F5 refresh · F6 other browser")}</>}>
                  <div className="wc-keys"><kbd>F2</kbd><kbd>F3</kbd><kbd>F5</kbd></div>
                </Choice>
                <Choice on={keyboardProfile(s) === "totalCommander"} onClick={() => set({ keyboardProfile: "totalCommander" })} title={tt("Total Commander")}
                  sub={<>{tt("F3 preview · F4 edit · F5 copy · F6 move · F7 folder · F8 trash · type to filter")}</>}>
                  <div className="wc-keys"><kbd>F5</kbd><kbd>F6</kbd><kbd>F8</kbd></div>
                </Choice>
                <Choice on={keyboardProfile(s) === "vim"} onClick={() => set({ keyboardProfile: "vim" })} title={tt("Vim")}
                  sub={<><kbd>j</kbd>/<kbd>k</kbd> move · <kbd>h</kbd>/<kbd>l</kbd> out and in · <kbd>/</kbd> filter · <kbd>{tt("Space")}</kbd> opens a menu of commands · <kbd>?</kbd> {tt("all keys")}</>}>
                  <div className="wc-keys vim"><kbd>h</kbd><kbd>j</kbd><kbd>k</kbd><kbd>l</kbd></div>
                </Choice>
              </div>
              <p className="wc-desc">{tt("All profiles keep custom shortcuts and Gall features. Explorer and Commander: tap Ctrl twice for the full command menu. Vim: press Space.")}</p>
            </>
          )}

          {cur.id === "files" && (
            <div className="wc-rows">
              <div className="wc-row">
                <span className="wc-row-text"><span>{tt("View")}</span><small>{tt("How items are listed")}</small></span>
                <ThemedSelect value={s.defaultView} onChange={(e) => set({ defaultView: e.target.value })}>
                  {VIEWS.map((v) => <option key={v.id} value={v.id}>{v.label}</option>)}
                </ThemedSelect>
              </div>
              <div className="wc-row">
                <span className="wc-row-text"><span>{tt("Sort by")}</span></span>
                <span className="wc-sort">
                  <ThemedSelect value={s.defaultSort} onChange={(e) => set({ defaultSort: e.target.value as Settings["defaultSort"] })}>
                    {[["name", "Name"], ["ext", "Type"], ["modified", "Date modified"], ["created", "Date created"], ["size", "Size"]].map(([k, l]) => <option key={k} value={k}>{l}</option>)}
                  </ThemedSelect>
                  <button className="btn" title={tt("Direction")} onClick={() => set({ defaultDescending: !s.defaultDescending })}>
                    {s.defaultDescending ? "Descending ↓" : "Ascending ↑"}
                  </button>
                </span>
              </div>
              <div className="wc-row">
                <span className="wc-row-text"><span>{tt("Grouping")}</span></span>
                <ThemedSelect value={s.group} onChange={(e) => set({ group: e.target.value as Settings["group"] })}>
                  {GROUP_MODES.map((m) => <option key={m} value={m}>{tt(GROUP_LABEL[m])}</option>)}
                </ThemedSelect>
              </div>
              <Toggle label={tt("Remember view and sort per folder")} hint="Otherwise each browser keeps one view and sort in every folder" checked={s.rememberFolderViews} set={(v) => set({ rememberFolderViews: v })} />
              <Toggle label={tt("Show hidden files")} hint="Dot files and hidden items" checked={s.showHidden} set={(v) => set({ showHidden: v })} />
              <Toggle label={tt("Open with a single click")} hint="Otherwise a double click opens, a single click selects" checked={s.singleClickOpen} set={(v) => set({ singleClickOpen: v })} />
            </div>
          )}

          {cur.id === "defaultFm" && (
            <>
              <div className="wc-choices two" data-welcome-default-fm>
                <Choice
                  on={!dfm?.isDefault}
                  disabled={dfmBusy}
                  onClick={() => void applyDefaultFm(false)}
                  title={keepOrRestoreTitle(dfm, activePlatform, tt)}
                  sub={tt("Leave your current system file manager in charge of opening folders.")}
                >
                  <DefaultFmPic gall={false} />
                </Choice>
                <Choice
                  on={!!dfm?.isDefault}
                  disabled={dfmBusy || dfm === null || (!dfm.canSet && !dfm.isDefault)}
                  onClick={() => void applyDefaultFm(true)}
                  title={dfm?.isDefault ? tt("Gall is your default") : tt("Set Gall as default")}
                  sub={dfm?.isDefault ? tt("Folders opened from other apps will open in Gall.") : tt("Use Gall whenever other apps open a folder.")}
                >
                  <DefaultFmPic gall={true} />
                </Choice>
              </div>
              <div className="wc-rows">
                <div className="wc-row">
                  <span className="wc-row-text">
                    <span>{tt("Current default file manager")}</span>
                    <small>{platformScopeNote(activePlatform, tt)}</small>
                  </span>
                  <span className="wc-dfm-status" role="status" data-welcome-dfm-status>
                    {dfmBusy
                      ? tt("Applying…")
                      : dfm === null
                        ? tt("Checking…")
                        : dfm.currentHandler ?? fallbackSystemManager(activePlatform, tt)}
                  </span>
                </div>
                {unavailable && (
                  <div className="wc-row">
                    <span className="wc-row-text"><small className="wc-dfm-warn" role="status">{unavailable}</small></span>
                  </div>
                )}
                {dfmError && (
                  <div className="wc-row">
                    <span className="wc-row-text"><small className="wc-dfm-error" role="alert">{dfmError}</small></span>
                  </div>
                )}
              </div>
            </>
          )}

          {cur.id === "favorites" && sources.length > 0 && <FavoritesImportPanel sources={sources} />}
        </div>

        <footer className="wc-foot">
          <button className="link-btn wc-skip" onClick={close}>{step === 0 ? "Skip, use the defaults" : "Skip the rest"}</button>
          <span style={{ flex: 1 }} />
          {step > 0 && <button className="btn" onClick={() => setStep(step - 1)}>{tt("Back")}</button>}
          <button className="btn primary" onClick={() => (last ? close() : setStep(step + 1))}>{last ? "Start using Gall" : "Next"}</button>
        </footer>
      </div>
    </div>
  );
}
