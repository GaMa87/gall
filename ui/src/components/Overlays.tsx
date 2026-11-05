// Modal overlays, context menu, notifications and task progress.
import { createPortal } from "react-dom";
import { DiffView } from "./DiffView";
import { DirCompare } from "./DirCompare";
import { SimilarityMatrix } from "./SimilarityMatrix";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/ipc";
import type { FileEntry, Template } from "../api/types";
import { formatEta } from "../core/eta";
import { formatDateTime, formatSize } from "../core/format";
import { primaryMod } from "../core/keymap";
import { basename, parent, sepOf } from "../core/path";
import * as A from "../state/actions";
import { controller } from "../state/controllers";
import { bumpNonce } from "../state/hooks";
import { useKeyLabels } from "../state/keyLabels";
import { AboutInfo } from "./AboutInfo";
import { BatchRename } from "./BatchRename";
import { FolderLink } from "./FolderLink";
import { Welcome } from "./Welcome";
import { getState, setState, useStore, type DialogSpec, type MenuItem, type NoteKind, type Overlay } from "../state/store";
import { FileIcon, Icon, TAG_COLORS } from "./Icon";
import { Preview, usePreviewInfo } from "./Preview";
import { SearchWindow } from "./Search";
import { TrashWindow } from "./Trash";
import { QuickOpen } from "./QuickOpen";
import { FindInFiles } from "./FindInFiles";
import { KeyHelp } from "./WhichKey";
import { SettingsDialog } from "./Settings";
import { OpenWith } from "./OpenWith";

/** Ref that focuses an element once when it appears. A function written inline in the
 *  JSX is new on every render, so React would call it again and pull the focus back to
 *  the window on each update (Properties re-renders while it counts a folder). */
const focusOnMount = (el: HTMLElement | null) => el?.focus();

// ───────── context menu ─────────

/** Whether Ctrl is held, from key events and from the modifier state of mouse moves
 *  (so it is right even when Ctrl was pressed before the menu opened). */
function useCtrlHeld() {
  const [held, setHeld] = useState(false);
  useEffect(() => {
    const k = (e: KeyboardEvent) => setHeld(e.ctrlKey);
    const m = (e: MouseEvent) => setHeld(e.ctrlKey);
    const off = () => setHeld(false);
    window.addEventListener("keydown", k, true);
    window.addEventListener("keyup", k, true);
    window.addEventListener("mousemove", m, true);
    window.addEventListener("blur", off);
    return () => {
      window.removeEventListener("keydown", k, true);
      window.removeEventListener("keyup", k, true);
      window.removeEventListener("mousemove", m, true);
      window.removeEventListener("blur", off);
    };
  }, []);
  return held;
}

function MenuList({ items, x, y, onClose, depth = 0, active = true, onBack, startKb = -1 }: {
  items: MenuItem[]; x: number; y: number; onClose: () => void; depth?: number;
  /** This menu takes the keys (the root, or a submenu opened with the keyboard). */
  active?: boolean;
  /** Leaves a submenu opened with the keyboard (Left / h / Esc). */
  onBack?: () => void;
  startKb?: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const ctrlHeld = useCtrlHeld();
  // Print the keys that do it in the current mode (vim keys / rebound chords).
  const keys = useKeyLabels();
  const [pos, setPos] = useState({ x, y });
  const [open, setOpen] = useState<number | null>(null);
  const [kb, setKb] = useState(startKb);
  const [subPos, setSubPos] = useState({ x: 0, y: 0 });
  // A submenu opened with the keyboard takes the keys until it is left.
  const [subKb, setSubKb] = useState(false);
  const openSub = (i: number, byKeys: boolean) => {
    const el = ref.current?.children[i] as HTMLElement | undefined;
    if (el) {
      const r = el.getBoundingClientRect();
      setSubPos({ x: r.right - 2, y: r.top - 4 });
    }
    setOpen(i);
    setSubKb(byKeys);
  };

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    // Layout size, not getBoundingClientRect: the opening animation scales the menu down,
    // which made it measure smaller and stick out of the window by a few pixels. Again
    // when its content changes size (items that load later).
    const place = () => setPos({
      x: Math.max(4, Math.min(x, window.innerWidth - el.offsetWidth - 4)),
      y: Math.max(4, Math.min(y, window.innerHeight - el.offsetHeight - 4)),
    });
    place();
    const ro = new ResizeObserver(place);
    ro.observe(el);
    return () => ro.disconnect();
  }, [x, y]);

  useEffect(() => {
    if (!active || subKb) return;
    const key = (e: KeyboardEvent) => {
      const usable = items.map((it, i) => (!it.separator && !it.disabled ? i : -1)).filter((i) => i >= 0);
      // Vim keys: j/k move, l runs the item or opens its submenu, h / q go back or close.
      const vim = (getState().settings?.vimKeys ?? false) && !e.ctrlKey && !e.altKey && !e.metaKey;
      const k = vim ? ({ j: "ArrowDown", k: "ArrowUp", l: "ArrowRight", h: "ArrowLeft", q: "Escape" } as Record<string, string>)[e.key] ?? e.key : e.key;
      const cur = kb >= 0 ? items[kb] : undefined;
      if (k === "Escape" || (k === "ArrowLeft" && (depth > 0 || vim))) {
        if (depth > 0 && onBack) onBack();
        else onClose();
      } else if (k === "ArrowDown") setKb((k) => usable[(usable.indexOf(k) + 1) % usable.length]);
      else if (k === "ArrowUp") setKb((k) => usable[(usable.indexOf(k) - 1 + usable.length) % usable.length]);
      else if ((k === "ArrowRight" || k === "Enter") && cur?.submenu) openSub(kb, true);
      else if ((k === "Enter" || (vim && k === "ArrowRight")) && cur) {
        (e.ctrlKey && cur.ctrl ? cur.ctrl.run : cur.run)?.();
        onClose();
      } else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [items, kb, depth, onClose, active, subKb, onBack]);

  return (
    <div className="menu" ref={ref} style={{ left: pos.x, top: pos.y }} onMouseDown={(e) => e.stopPropagation()} onContextMenu={(e) => e.preventDefault()}>
      {items.map((it, i) =>
        it.separator ? (
          <div key={i} className="sep" />
        ) : (
          <div
            key={i}
            className={`mi ${it.disabled ? "disabled" : ""} ${it.danger ? "danger" : ""} ${kb === i ? "kb" : ""}`}
            onMouseEnter={(e) => {
              setKb(i);
              if (it.submenu) {
                const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
                setSubPos({ x: r.right - 2, y: r.top - 4 });
                setOpen(i);
              } else setOpen(null);
              setSubKb(false);
            }}
            title={it.ctrl ? `Hold Ctrl: ${it.ctrl.label}` : undefined}
            onClick={(e) => {
              if (it.disabled || it.submenu) return;
              onClose();
              (e.ctrlKey && it.ctrl ? it.ctrl.run : it.run)?.();
            }}
          >
            <span style={{ width: 16, display: "inline-flex" }}>{it.swatch ? <span className={`swatch ${it.checked ? "on" : ""}`} style={{ background: it.swatch }} /> : it.checked ? <Icon name="check" size={14} /> : it.icon ? <Icon name={it.icon} size={14} /> : null}</span>
            <span className="lbl">{ctrlHeld && it.ctrl ? it.ctrl.label : it.label}</span>
            {(() => {
              const sc = it.action ? keys.action(it.action) : it.shortcut ? keys.chord(it.shortcut) : null;
              return sc && <span className="sc">{sc}</span>;
            })()}
            {it.submenu && <Icon name="chevronRight" size={12} />}
            {/* Portal: .menu has backdrop-filter, which would make a nested position:fixed
                submenu position itself relative to the parent menu instead of the window. */}
            {it.submenu && open === i && createPortal(
              <MenuList items={it.submenu} x={subPos.x} y={subPos.y} onClose={onClose} depth={depth + 1}
                active={subKb} onBack={() => { setSubKb(false); setOpen(null); }}
                startKb={subKb ? it.submenu.findIndex((s) => !s.separator && !s.disabled) : -1} />,
              document.body)}
          </div>
        ),
      )}
    </div>
  );
}

export function ContextMenu() {
  const menu = useStore((s) => s.menu);
  useEffect(() => {
    if (!menu) return;
    const close = () => setState({ menu: null });
    window.addEventListener("mousedown", close);
    window.addEventListener("blur", close);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("resize", close);
    };
  }, [menu]);
  if (!menu) return null;
  return <MenuList items={menu.items} x={menu.x} y={menu.y} onClose={() => setState({ menu: null })} />;
}

// ───────── generic dialog ─────────

/** Button label with its accelerator letter underlined (first occurrence, any case). */
function AccelLabel({ label, k }: { label: string; k: string }) {
  const i = label.toLowerCase().indexOf(k.toLowerCase());
  if (i < 0) return <>{label}</>;
  return <>{label.slice(0, i)}<u className="accel">{label[i]}</u>{label.slice(i + 1)}</>;
}

const dialogKeys = new WeakMap<DialogSpec, number>();
let dialogSeq = 0;
function dialogKey(d: DialogSpec): number {
  let k = dialogKeys.get(d);
  if (k == null) dialogKeys.set(d, (k = ++dialogSeq));
  return k;
}

function Dialog({ d }: { d: DialogSpec }) {
  const [value, setValue] = useState(d.input?.value ?? "");
  const inputRef = useRef<HTMLInputElement>(null);
  const toggle = useRef(0);
  // The window the dialog was asked from (it stays open beneath) gets the keyboard back.
  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    return () => { if (prev && prev !== document.body && prev.isConnected) prev.focus(); };
  }, []);
  useEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.focus();
    const [a, b] = d.input?.select ?? [0, el.value.length];
    el.setSelectionRange(a, b);
  }, [d]);
  const primary = d.buttons.find((b) => b.primary) ?? d.buttons[d.buttons.length - 1];
  const finish = (id: string | null) => d.resolve(id === null ? null : { button: id, value });
  const buttonsRef = useRef<HTMLDivElement>(null);
  const vim = useStore((s) => s.settings?.vimKeys ?? false);
  const focusedButton = () => (document.activeElement as HTMLElement | null)?.closest?.(".buttons [data-id]") as HTMLElement | null;
  return (
    <div className="backdrop" onMouseDown={() => finish(null)}>
      <div className="modal" onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          e.stopPropagation();
          // Enter / Esc that end an input method's composition (Japanese, Chinese…) belong
          // to it: they submitted or cancelled the dialog with the half-typed name.
          if (e.nativeEvent.isComposing) return;
          const plain = !e.ctrlKey && !e.altKey && !e.metaKey;
          // Vim keys (no text field to type into): h/l move between buttons, q cancels.
          const vimKey = vim && plain && !d.input ? e.key : "";
          const left = e.key === "ArrowLeft" || vimKey === "h";
          const right = e.key === "ArrowRight" || vimKey === "l";
          const accel = plain && !d.input && e.key.length === 1 ? d.buttons.find((b) => b.key === e.key.toLowerCase()) : undefined;
          if (e.key === "Escape" || (vimKey === "q" && !accel)) finish(null);
          else if ((left || right) && plain && !d.input) {
            // Move focus between the buttons; Enter then presses the focused one.
            e.preventDefault();
            const btns = [...(buttonsRef.current?.querySelectorAll<HTMLElement>("[data-id]") ?? [])];
            const cur = btns.indexOf(focusedButton()!);
            const from = cur < 0 ? btns.findIndex((x) => x.dataset.id === primary.id) : cur;
            btns[(from + (left ? -1 : 1) + btns.length) % btns.length]?.focus();
          } else if (accel) {
            e.preventDefault();
            finish(accel.id);
          } else if (e.key === "Enter") {
            e.preventDefault();
            const fb = focusedButton();
            if (fb && plain) finish(fb.dataset.id!);
            else if (primaryMod(e) && d.buttons.some((b) => b.id === "open")) finish("open");
            else if (e.shiftKey && d.buttons.some((b) => b.id === "moveIn")) finish("moveIn");
            else finish(primary.id);
          } else if (e.key === "F2" && d.input?.toggleSelections?.length && inputRef.current) {
            e.preventDefault();
            const sels = d.input.toggleSelections;
            toggle.current = (toggle.current + 1) % sels.length;
            inputRef.current.setSelectionRange(...sels[toggle.current]);
          }
        }}
        tabIndex={-1} ref={d.input ? undefined : focusOnMount}>
        <h3>{d.title}</h3>
        <div className="body">
          {d.message && <p>{d.message}</p>}
          {d.input && <input ref={inputRef} type={d.input.password ? "password" : "text"} autoComplete="off" value={value} placeholder={d.input.placeholder} onChange={(e) => setValue(e.target.value)} spellCheck={false} />}
        </div>
        <div className="buttons" ref={buttonsRef}>
          {d.buttons.map((b) => (
            <button key={b.id} data-id={b.id} className={`btn ${b.primary ? "primary" : ""} ${b.danger ? "danger" : ""}`} onClick={() => finish(b.id)}
              title={b.key && !d.input ? `${b.label} (${b.key.toUpperCase()})` : undefined}>
              {b.key && !d.input ? <AccelLabel label={b.label} k={b.key} /> : b.label}{b.hint && <small>{b.hint}</small>}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

// ───────── path edit (N-09, N-10) ─────────

function PathEdit({ b }: { b: number }) {
  const current = A.activePath(b);
  const favorites = useStore((s) => s.favorites);
  const history = useStore((s) => s.history);
  const session = useStore((s) => s.session)!;
  const [value, setValue] = useState(current);
  const [completions, setCompletions] = useState<string[]>([]);
  const [clip, setClip] = useState<string | null>(null);
  const [sel, setSel] = useState(-1);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.select();
    api.clipInspect().then((c) => c.kind === "paths" && setClip(c.paths[0])).catch(() => {});
  }, []);
  useEffect(() => {
    if (!value || value === current) return setCompletions([]);
    // A reply for what was typed before is dropped (it would list the wrong folder).
    let alive = true;
    const t = setTimeout(() => api.pathComplete(value).then((c) => alive && setCompletions(c)).catch(() => {}), 80);
    return () => { alive = false; clearTimeout(t); };
  }, [value, current]);

  const suggestions = useMemo(() => {
    const q = value === current ? "" : value.toLowerCase();
    const out: { path: string; hint: string }[] = [];
    const add = (p: string, hint: string) => {
      if (!out.some((o) => o.path === p)) out.push({ path: p, hint });
    };
    completions.forEach((p) => add(p, "folder"));
    if (clip) add(clip, "clipboard");
    const tabs = session.browsers.flatMap((br) => br.tabs.map((t) => t.path));
    const favs = favorites.groups.flatMap((g) => g.items.filter((f) => !f.path.startsWith("trash:")).map((f) => f.path));
    const match = (p: string) => !q || p.toLowerCase().includes(q) || basename(p).toLowerCase().includes(q);
    favs.filter(match).forEach((p) => add(p, "favourite"));
    tabs.filter(match).forEach((p) => add(p, "tab"));
    history.filter(match).forEach((p) => add(p, "recent"));
    if (!q) {
      let p = parent(current);
      while (p) {
        add(p, "parent");
        p = parent(p);
      }
    }
    return out.slice(0, 40);
  }, [value, current, completions, clip, favorites, history, session]);

  const go = (p: string) => {
    A.openOverlay(null);
    A.goToTyped(b, p);
  };

  return (
    <div className="backdrop" onMouseDown={() => A.openOverlay(null)}>
      <div className="modal path-edit" onMouseDown={(e) => e.stopPropagation()} tabIndex={-1}>
        <div className="body" style={{ paddingTop: 14 }}>
          <input
            ref={inputRef}
            type="text"
            value={value}
            spellCheck={false}
            onChange={(e) => { setValue(e.target.value); setSel(-1); }}
            onContextMenu={(e) => {
              e.preventDefault();
              A.showMenu(e.clientX, e.clientY, (
                [["native", "Copy path"], ["nativeQuoted", "Copy path in quotes"], ["escaped", "Copy with escaped backslashes"], ["forward", "Copy with forward slashes"], ["forwardQuoted", "Copy with forward slashes, quoted"], ["uri", "Copy as file:// URI"], ["linux", "Copy as Linux path (WSL)"]] as const
              ).filter(([f]) => f !== "linux" || getState().info?.platform === "windows").map(([f, l]) => ({ label: l, run: () => api.pathFormat(current, f).then((t) => api.clipSetText(t)).then(() => A.notify("Path copied")).catch(A.fail) })));
            }}
            onKeyDown={(e) => {
              e.stopPropagation();
              if (e.nativeEvent.isComposing) return; // the input method's Enter / Esc
              if (e.key === "Escape") A.openOverlay(null);
              else if (e.key === "ArrowDown") { e.preventDefault(); setSel((s) => Math.min(suggestions.length - 1, s + 1)); }
              else if (e.key === "ArrowUp") { e.preventDefault(); setSel((s) => Math.max(-1, s - 1)); }
              // The list can shrink under the highlight (completions and the clipboard arrive
              // later): an index past its end means none.
              else if (e.key === "Tab" && suggestions[0]) { e.preventDefault(); { const p = (suggestions[sel] ?? suggestions[0]).path; setValue(p.endsWith(sepOf(p)) ? p : p + sepOf(p)); } }
              else if (e.key === "Enter") go(suggestions[sel]?.path ?? value);
              else if (e.key === "c" && primaryMod(e) && inputRef.current?.selectionStart === inputRef.current?.selectionEnd) {
                api.clipSetText(value).then(() => A.notify("Path copied")).catch(A.fail);
              }
            }}
          />
          <div className="suggest">
            {suggestions.map((s, i) => (
              <div key={s.path} className={`s ${i === sel ? "on" : ""}`} onClick={() => go(s.path)} title={s.path}>
                <Icon name="folder" size={14} color="var(--folder)" />
                <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{s.path}</span>
                <small>{s.hint}</small>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

// ───────── templates (O-08) ─────────

function Templates({ dir, b }: { dir: string; b: number }) {
  const info = useStore((s) => s.info);
  const [list, setList] = useState<Template[]>([]);
  const [sel, setSel] = useState(0);
  const [name, setName] = useState("");
  const nameRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    // Without the templates (folder unreadable) an empty file can still be made: the list
    // stayed empty and the name blank, so the window could not create anything.
    const empty: Template = { name: "Empty file", path: "", suggested: "New file.txt" };
    api.templatesList().catch((e) => (A.fail(e), [] as Template[])).then((l) => {
      setList([empty, ...l]);
      setName(empty.suggested);
    });
    listRef.current?.focus();
  }, []);
  const pick = (i: number) => {
    setSel(i);
    if (list[i]) setName(list[i].suggested);
  };
  const create = (open: boolean) => {
    if (!name.trim()) return;
    A.openOverlay(null);
    A.createFromTemplate(b, dir, list[sel]?.path || undefined, name.trim(), open);
  };
  return (
    <div className="backdrop" onMouseDown={() => A.openOverlay(null)}>
      <div className="modal" style={{ width: 460 }} onMouseDown={(e) => e.stopPropagation()} tabIndex={-1}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.nativeEvent.isComposing) return; // the input method's Enter / Esc
          if (e.key === "Escape") A.openOverlay(null);
          if (e.key === "Enter") { e.preventDefault(); create(primaryMod(e)); }
        }}>
        <h3>New file</h3>
        <div className="body">
          <div ref={listRef} tabIndex={0} className="suggest" style={{ maxHeight: 260, marginTop: 0, outline: "none" }}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown") { e.preventDefault(); pick(Math.min(list.length - 1, sel + 1)); }
              if (e.key === "ArrowUp") { e.preventDefault(); pick(Math.max(0, sel - 1)); }
              if (e.key === "Tab") { e.preventDefault(); nameRef.current?.focus(); const [s] = name.split("."); nameRef.current?.setSelectionRange(0, s.length); }
            }}>
            {list.map((t, i) => (
              <div key={t.path || "empty"} className={`s ${i === sel ? "on" : ""}`} onClick={() => pick(i)} onDoubleClick={() => create(false)}>
                <FileIcon entry={{ isDir: false, name: t.name, ext: t.name.split(".").pop()?.toLowerCase() ?? "" }} />
                {t.name}
              </div>
            ))}
          </div>
          <p style={{ margin: "10px 0 4px" }}>File name</p>
          <input ref={nameRef} type="text" value={name} onChange={(e) => setName(e.target.value)} spellCheck={false} />
        </div>
        <div className="buttons">
          <FolderLink path={info?.templatesDir ?? ""} label="Templates folder" />
          <span style={{ flex: 1 }} />
          <button className="btn" onClick={() => create(true)}>Create & open<small>Ctrl+Enter</small></button>
          <button className="btn primary" onClick={() => create(false)}>Create</button>
        </div>
      </div>
    </div>
  );
}

// ───────── note editor (O-09) ─────────

function NoteEditor({ dir, path, text, overlay }: {
  dir: string;
  path?: string;
  text?: string;
  overlay: Extract<Overlay, { type: "note" }>;
}) {
  const [value, setValue] = useState(text ?? "");
  const [notePath, setNotePath] = useState(path);
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const notePathRef = useRef(path);
  const valueRef = useRef(value);
  valueRef.current = value;
  const save = async () => {
    // A slow write must not start a second create/update, but edits made while it is
    // pending remain in the editor and can be saved again after this request completes.
    if (savingRef.current) return;
    savingRef.current = true;
    setSaving(true);
    const submitted = valueRef.current;
    let succeeded = false;
    try {
      if (notePathRef.current) await api.updateNote(notePathRef.current, submitted);
      else if (submitted.trim()) {
        const createdPath = await api.createNote(dir, submitted);
        notePathRef.current = createdPath;
        setNotePath(createdPath);
      }
      succeeded = true;
    } catch (e) {
      A.fail(e);
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
    if (!succeeded) return;
    void A.refreshUndo();
    // Do not discard edits typed during the write, or close an overlay opened since.
    if (getState().overlay === overlay && valueRef.current === submitted) A.openOverlay(null);
  };
  return (
    <div className="backdrop" onMouseDown={() => A.openOverlay(null)}>
      <div className="modal" style={{ width: 520 }} onMouseDown={(e) => e.stopPropagation()} tabIndex={-1}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229) return;
          if (e.key === "Escape") A.openOverlay(null);
          if (e.key === "Enter" && primaryMod(e)) { e.preventDefault(); void save(); }
        }}>
        <h3>{notePath ? `Edit note — ${basename(notePath)}` : "New note"}</h3>
        <div className="body">
          <textarea autoFocus rows={8} value={value} onChange={(e) => {
            valueRef.current = e.target.value;
            setValue(e.target.value);
          }} placeholder="First line becomes the file name…" />
        </div>
        <div className="buttons">
          <button className="btn" onClick={() => A.openOverlay(null)}>Cancel</button>
          <button className="btn primary" disabled={saving} onClick={() => void save()}>{saving ? "Saving…" : "Save"}<small>Ctrl+Enter</small></button>
        </div>
      </div>
    </div>
  );
}

// ───────── colour tags (O-14) ─────────

function TagPopup({ paths }: { paths: string[] }) {
  const apply = (c: number) => {
    api.setTag(paths, c).then(bumpNonce).catch(A.fail);
    A.openOverlay(null);
  };
  return (
    <div className="backdrop" onMouseDown={() => A.openOverlay(null)}>
      <div className="modal" tabIndex={-1} ref={focusOnMount} onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") A.openOverlay(null);
          // Digits only: Number(" ") is 0, so Space (pressing a focused colour) removed the tag.
          if (/^[0-9]$/.test(e.key) && Number(e.key) < TAG_COLORS.length) { e.preventDefault(); apply(Number(e.key)); }
        }}>
        <h3>Colour tag ({paths.length} item{paths.length > 1 ? "s" : ""})</h3>
        <div className="body">
          <p>Press a number key or click a colour. 0 removes the tag.</p>
          <div className="tags">
            {TAG_COLORS.map((c, i) => (
              <button key={i} style={{ background: c || "var(--bg-3)", color: c ? "#fff" : "var(--fg)" }} onClick={() => apply(i)}>{i}</button>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

// ───────── properties (O-13) ─────────

function Properties({ entry }: { entry: FileEntry }) {
  const info = usePreviewInfo(entry);
  // A folder's total size is counted while the window is open.
  const total = useStore((s) => (entry.isDir ? s.folderSizes[entry.path] : undefined));
  useEffect(() => {
    if (!entry.isDir) return;
    A.calcFolderSizes("properties", [entry.path], 30_000);
    return () => A.cancelFolderSizes("properties");
  }, [entry.path, entry.isDir]);
  return (
    <div className="backdrop center" onMouseDown={() => A.openOverlay(null)}>
      <div className="modal" style={{ width: 480 }} onMouseDown={(e) => e.stopPropagation()} tabIndex={-1} ref={focusOnMount}
        onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Escape" || e.key === "Enter") A.openOverlay(null); }}>
        <h3 style={{ display: "flex", gap: 8, alignItems: "center" }}><FileIcon entry={entry} size={22} /> {entry.name}</h3>
        <div className="body props">
          <table>
            <tbody>
              <tr><td>Location</td><td>{parent(entry.path)}</td></tr>
              <tr><td>Type</td><td>{entry.isDir ? "Folder" : entry.ext ? `${entry.ext.toUpperCase()} file` : "File"}{entry.isSymlink ? " (symbolic link)" : ""}</td></tr>
              {!entry.isDir && <tr><td>Size</td><td>{formatSize(entry.size)} ({entry.size.toLocaleString()} bytes)</td></tr>}
              {entry.isDir && (
                <tr><td>Size</td><td title={A.folderSizeTitle(total)}>
                  {total ? `${A.folderSizeLabel(total)} (${total.bytes.toLocaleString()} bytes) in ${total.files.toLocaleString()} files, ${total.dirs.toLocaleString()} folders${total.unreadable ? `; ${A.items(total.unreadable)} unreadable` : ""}` : "counting…"}
                </td></tr>
              )}
              {entry.isDir && info?.folderItems != null && <tr><td>Contents</td><td>{A.items(info.folderItems)} directly inside</td></tr>}
              {info?.width && <tr><td>Dimensions</td><td>{info.width} × {info.height}</td></tr>}
              <tr><td>Created</td><td>{formatDateTime(entry.created)}</td></tr>
              <tr><td>Modified</td><td>{formatDateTime(entry.modified)}</td></tr>
              <tr><td>Attributes</td><td>{[entry.readonly && "read-only", entry.isHidden && "hidden"].filter(Boolean).join(", ") || "—"}</td></tr>
            </tbody>
          </table>
        </div>
        <div className="buttons">
          <button className="btn" onClick={() => A.copyPaths([entry], entry.path)}>Copy path</button>
          <button className="btn primary" onClick={() => A.openOverlay(null)}>Close</button>
        </div>
      </div>
    </div>
  );
}

// ───────── preview window (P-06) ─────────

function PreviewWindow({ b }: { b: number }) {
  const entry = useStore((s) => s.focused[b]);
  const info = usePreviewInfo(entry);
  const vim = useStore((s) => s.settings?.vimKeys ?? false);
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (dialogOnTop()) return;
      if (e.key === "Escape" || e.key === " ") {
        if ((e.target as HTMLElement).tagName === "VIDEO" && e.key === " ") return;
        e.preventDefault();
        e.stopPropagation();
        A.openOverlay(null);
        controller(b)?.focus();
      } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        e.stopPropagation();
        const c = controller(b);
        if (!c) return;
        const list = c.entries();
        const cur = c.focused();
        const i = cur ? list.findIndex((x) => x.path === cur.path) : -1;
        const next = list[Math.max(0, Math.min(list.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))];
        if (next) c.select([next.path], next.path);
      } else scrollPreview(e);
    };
    // Scrolling the document (text, Markdown, PDF, archive listing). Vim: j/k line,
    // Ctrl+d/u half page, Ctrl+f/b page, gg/G ends; always PageUp/PageDown/Home/End.
    // Media players keep their own keys (k = play/pause): nothing scrollable there.
    let gAt = 0;
    const scrollPreview = (e: KeyboardEvent) => {
      const el = document.querySelector<HTMLElement>(".pw .pb :is(.pv-md, .pv-text, .pv-pdf, .pv-archive, .pv-note)");
      if (!el || e.altKey || e.metaKey) return;
      const vim = getState().settings?.vimKeys ?? false;
      const k = e.key, ctrl = e.ctrlKey;
      // ←/→ turn PDF pages even when the PDF has not been clicked (focused) yet.
      if (el.classList.contains("pv-pdf") && (k === "ArrowLeft" || k === "ArrowRight") && !ctrl && !el.contains(e.target as Node)) {
        e.preventDefault();
        e.stopPropagation();
        el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true }));
        return;
      }
      const line = 60, half = el.clientHeight / 2, page = el.clientHeight * 0.9;
      let dy: number | "top" | "bottom" | null = null;
      if (k === "PageDown" && !ctrl) dy = page;
      else if (k === "PageUp" && !ctrl) dy = -page;
      else if (k === "Home" && !ctrl) dy = "top";
      else if (k === "End" && !ctrl) dy = "bottom";
      else if (vim && !ctrl && k === "j") dy = line;
      else if (vim && !ctrl && k === "k") dy = -line;
      else if (vim && ctrl && k === "d") dy = half;
      else if (vim && ctrl && k === "u") dy = -half;
      else if (vim && ctrl && k === "f") dy = page;
      else if (vim && ctrl && k === "b") dy = -page;
      else if (vim && !ctrl && k === "G") dy = "bottom";
      else if (vim && !ctrl && k === "g") {
        const now = performance.now();
        if (now - gAt < 500) { dy = "top"; gAt = 0; } else { gAt = now; e.preventDefault(); e.stopPropagation(); return; }
      }
      if (dy === null) return;
      e.preventDefault();
      e.stopPropagation();
      const max = el.scrollHeight - el.clientHeight;
      // PDF: past the end of a page continue on the next one (and back).
      if (el.classList.contains("pv-pdf") && typeof dy === "number") {
        const atEnd = dy > 0 ? el.scrollTop >= max - 1 : el.scrollTop <= 0;
        const pg = Number(el.dataset.page), pages = Number(el.dataset.pages);
        const canTurn = dy > 0 ? pg < pages : pg > 1;
        if (atEnd && !canTurn) return;
        if (atEnd) {
          el.dispatchEvent(new KeyboardEvent("keydown", { key: dy > 0 ? "ArrowRight" : "ArrowLeft", bubbles: true }));
          requestAnimationFrame(() => { el.scrollTop = dy > 0 ? 0 : el.scrollHeight; });
          return;
        }
      }
      if (dy === "top") el.scrollTo({ top: 0 });
      else if (dy === "bottom") el.scrollTo({ top: max });
      else el.scrollBy({ top: dy, behavior: Math.abs(dy) > line ? "smooth" : "auto" });
    };
    window.addEventListener("keydown", key, true);
    return () => window.removeEventListener("keydown", key, true);
  }, [b]);
  if (!entry) return null;
  return (
    <div className="backdrop center" style={{ padding: 0 }} onMouseDown={() => A.openOverlay(null)}>
      <div className="pw" onMouseDown={(e) => e.stopPropagation()} tabIndex={-1}>
        <div className="ph">
          <FileIcon entry={entry} />
          <span className="t">{entry.name}</span>
          <span style={{ color: "var(--fg-dim)" }}>{entry.isDir ? "" : formatSize(entry.size)}</span>
          <button className="icon-btn" title="Open" onClick={() => api.openPath(entry.path).catch(A.fail)}><Icon name="forward" /></button>
          <button className="icon-btn" title="Close (Esc)" onClick={() => A.openOverlay(null)}><Icon name="close" /></button>
        </div>
        <div className="pb"><Preview entry={entry} info={info} big /></div>
        <PreviewHints kind={info?.kind ?? null} vim={vim} />
      </div>
    </div>
  );
}

/** Bottom bar of the preview window: the keys that work for this kind of file. */
function PreviewHints({ kind, vim }: { kind: string | null; vim: boolean }) {
  const scrolls = kind === "text" || kind === "pdf" || kind === "archive" || kind === "note";
  const hints: [string[], string][] = [[["↑", "↓"], "file"]];
  if (scrolls) {
    if (vim) hints.push([["j", "k"], "scroll"], [["Ctrl+d", "Ctrl+u"], "half page"], [["gg", "G"], "top / end"]);
    else hints.push([["PgDn", "PgUp"], "page"], [["Home", "End"], "top / end"]);
  }
  if (kind === "pdf") hints.push([["←", "→"], "page"]);
  if (kind === "video" || kind === "audio") hints.push([["←", "→"], "seek"], [["[", "]"], "frame"], [["k"], "play / pause"]);
  hints.push([["Esc", "q"], "close"]);
  return (
    <div className="pw-foot">
      {hints.map(([keys, label]) => (
        <span key={label + keys.join()} className="h">{keys.map((k) => <kbd key={k}>{k}</kbd>)}<span>{label}</span></span>
      ))}
    </div>
  );
}

// ───────── about ─────────

function About() {
  return (
    <div className="backdrop center" onMouseDown={() => A.openOverlay(null)}>
      <div className="modal about-win" onMouseDown={(e) => e.stopPropagation()} tabIndex={-1} ref={focusOnMount}
        onKeyDown={(e) => { e.stopPropagation(); if (e.key === "Escape") A.openOverlay(null); }}>
        <button className="icon-btn about-close" title="Close (Esc)" onClick={() => A.openOverlay(null)}><Icon name="close" size={14} /></button>
        <div className="body">
          <AboutInfo />
        </div>
      </div>
    </div>
  );
}

/** Overlays where q is not "close": dialogs give letters to their buttons (and handle q
 *  themselves), the note editor is all text. */
const Q_KEEPS = new Set<Overlay["type"]>(["dialog", "note"]);

export function Overlays() {
  const o = useStore((s) => s.overlay);
  // q closes like Esc (as in lazygit, less, man), unless a text field has the keyboard:
  // the q is turned into an Escape on the same element, so each overlay closes its own way.
  useEffect(() => {
    if (!o || Q_KEEPS.has(o.type)) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "q" || e.ctrlKey || e.altKey || e.metaKey) return;
      const t = e.target as HTMLElement;
      if (t.isContentEditable || t.closest("input, textarea, select")) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      t.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", code: "Escape", bubbles: true, cancelable: true }));
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [o]);
  if (!o) return null;
  // A dialog asked from a window is drawn over it: the window keeps its place in the tree
  // (and so its state, e.g. unsaved merges in the compare view) while the dialog is open.
  const base = o.type === "dialog" ? o.under : o;
  return (
    <>
      {base && <OverlayWindow o={base} />}
      {/* A fresh dialog for each question: the focus starts on it, not on a button of the last one. */}
      {o.type === "dialog" && <Dialog key={dialogKey(o.dialog)} d={o.dialog} />}
    </>
  );
}

/** A dialog is open over the window that asked it (its window-wide keys must wait). */
const dialogOnTop = () => getState().overlay?.type === "dialog";

function OverlayWindow({ o }: { o: Overlay }) {
  switch (o.type) {
    case "dialog": return null;
    case "pathEdit": return <PathEdit b={o.b} />;
    case "templates": return <Templates dir={o.dir} b={o.b} />;
    case "note": return <NoteEditor dir={o.dir} path={o.path} text={o.text} overlay={o} />;
    case "tag": return <TagPopup paths={o.paths} />;
    case "properties": return <Properties entry={o.entry} />;
    case "batchRename": return <BatchRename b={o.b} entries={o.entries} />;
    case "previewWindow": return <PreviewWindow b={o.b} />;
    case "search": return <SearchWindow root={o.root} initialPattern={o.pattern} initialEverything={o.everything} />;
    case "trash": return <TrashWindow />;
    case "quickOpen": return <QuickOpen initial={o.initial} />;
    case "grep": return <FindInFiles root={o.root} scope={o.scope} />;
    case "keys": return <KeyHelp />;
    case "settings": return <SettingsDialog section={o.section} />;
    case "about": return <About />;
    case "welcome": return <Welcome />;
    // Keyed by its sources: DiffView keeps them in state, so a new pair needs a fresh instance.
    case "diff": return <DiffView key={JSON.stringify([o.a, o.b])} a={o.a} b={o.b} back={o.back} />;
    case "dircmp": return <DirCompare left={o.left} right={o.right} />;
    case "matrix": return <SimilarityMatrix paths={o.paths} />;
    case "openWith": return <OpenWith paths={o.paths} mode={o.mode} />;
  }
}

// ───────── notifications + tasks (W-09, O-16) ─────────

const NOTE_ICON: Record<NoteKind, string> = { info: "info", success: "check", warn: "alert", error: "error" };

export function Notifications() {
  const notes = useStore((s) => s.notifications);
  const log = useStore((s) => s.notificationLog);
  const showLog = useStore((s) => s.showNotificationLog);
  const tasks = useStore((s) => s.tasks);
  return (
    <>
      <div className="toasts" onMouseEnter={() => A.holdNotes(true)} onMouseLeave={() => A.holdNotes(false)}>
        {/* Running file operations sit with the notifications, styled the same, above them. */}
        {Object.values(tasks).map((t) => {
          const pct = t.totalBytes ? (t.doneBytes / t.totalBytes) * 100 : t.totalItems ? (t.doneItems / t.totalItems) * 100 : 0;
          return (
            <div key={t.id} className="toast task">
              <div className="line">
                <b>{{ copy: "Copying", move: "Moving", delete: "Deleting", compress: "Compressing", extract: "Extracting", duplicate: "Duplicating" }[t.kind]}</b>
                <span className="eta" title="Estimated time left">{t.eta === null ? "estimating…" : `${formatEta(t.eta)} left`}</span>
                <span className="pct">{Math.floor(pct)}%</span>
                <button className="icon-btn" title="Cancel" onClick={() => api.cancelTask(t.id)}><Icon name="close" size={12} /></button>
              </div>
              <div className="bar"><div style={{ width: `${pct}%` }} /></div>
              <div className="line dim">
                <span className="cur">{basename(t.current)}</span>
                <span>{t.doneItems}/{t.totalItems}{t.totalBytes > 0 && <> · {formatSize(t.doneBytes)} / {formatSize(t.totalBytes)}</>}</span>
              </div>
            </div>
          );
        })}
        {notes.map((n) => (
          <div key={n.id} className={`toast ${n.kind}`} role={n.kind === "error" ? "alert" : "status"} onClick={() => A.dismiss(n.id)}>
            <span className="ti"><Icon name={NOTE_ICON[n.kind]} size={n.kind === "error" ? 18 : 16} /></span>
            <span className="t">{n.text}</span>
            {n.action && <button onClick={(e) => { e.stopPropagation(); n.action!.run(); A.dismiss(n.id); }}>{n.action.label}</button>}
            {n.kind === "error" && <span className="tx" title="Dismiss"><Icon name="close" size={12} /></span>}
          </div>
        ))}
      </div>
      {showLog && (
        <div className="notif-log" onMouseDown={(e) => e.stopPropagation()}>
          {log.length ? log.map((n) => (
            <div key={n.id} className="n"><small>{new Date(n.at).toLocaleTimeString()}</small>{n.text}</div>
          )) : <div className="n">No notifications yet</div>}
        </div>
      )}
    </>
  );
}
