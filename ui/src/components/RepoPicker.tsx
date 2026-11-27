import { useEffect, useRef, useState } from "react";
import { dropdownBox, type DropdownBox } from "../core/anchor";
import { HoverGuard } from "../core/hover";
import { tt } from "../i18n";
import "./RepoPicker.css";

export type RepoPickerEntry = { label: string; path: string; state: string; depth: number; name: string };

type MarkKind = "main" | "submodule" | "nested";
const DIAMOND = "M8 1.8 14.2 8 8 14.2 1.8 8Z";
/** Icon in front of a repository: solid for the opened one, outlined for a submodule, outlined with a core for a submodule of a submodule. */
function Mark({ kind, depth }: { kind: MarkKind; depth: number }) {
  return (
    <svg className={`rp-mark depth-${Math.min(depth, 2)}`} viewBox="0 0 16 16" width="13" height="13" aria-hidden>
      <path d={DIAMOND} fill={kind === "main" ? "currentColor" : "none"} stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" />
      {kind === "nested" && <path d="M8 5.4 10.6 8 8 10.6 5.4 8Z" fill="currentColor" />}
    </svg>
  );
}

type Item = { path: string; name: string; depth: number; mark: MarkKind; hint: string; title: string; disabled: boolean };

/**
 * Repository selector styled like the app's menus: a tree with guide lines per level,
 * a "main" badge on the opened repository and a muted note on submodules that cannot be opened.
 */
export function RepoPicker({ value, mainPath, mainName, entries, disabled, vim, onSelect }: {
  value: string; mainPath: string; mainName: string; entries: RepoPickerEntry[]; disabled?: boolean; vim: boolean; onSelect: (path: string) => void;
}) {
  const [open, setOpen] = useState(false), [active, setActive] = useState(0), [box, setBox] = useState<DropdownBox | null>(null);
  const button = useRef<HTMLButtonElement>(null), list = useRef<HTMLDivElement>(null);
  const hover = useRef(new HoverGuard(false));
  const items: Item[] = [
    { path: mainPath, name: mainName, depth: 0, mark: "main", hint: tt("main"), title: mainName, disabled: false },
    ...entries.map((entry): Item => ({
      path: entry.path, name: entry.name, depth: entry.depth + 1, mark: entry.depth ? "nested" : "submodule", title: entry.label,
      hint: entry.state === "notInitialized" ? tt("not initialized") : entry.state === "unsupportedPath" ? tt("unsupported path") : "",
      disabled: entry.state === "notInitialized" || entry.state === "unsupportedPath",
    })),
  ];
  const current = items.find((item) => item.path === value) ?? items[0];
  const enabled = (from: number, step: number) => { for (let i = from; i >= 0 && i < items.length; i += step) if (!items[i].disabled) return i; return -1; };
  const show = () => {
    const rect = button.current?.getBoundingClientRect(); if (!rect || disabled) return;
    setBox(dropdownBox(rect, { minWidth: 240, maxHeight: window.innerHeight, gap: 4 }, window.innerWidth, window.innerHeight));
    hover.current.reset();
    setActive(Math.max(0, items.findIndex((item) => item.path === value))); setOpen(true);
  };
  const close = () => setOpen(false);
  const choose = (item: Item) => { if (item.disabled) return; close(); button.current?.focus(); onSelect(item.path); };
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => { if (!list.current?.contains(e.target as Node) && !button.current?.contains(e.target as Node)) close(); };
    window.addEventListener("mousedown", away, true); window.addEventListener("resize", close); window.addEventListener("blur", close);
    return () => { window.removeEventListener("mousedown", away, true); window.removeEventListener("resize", close); window.removeEventListener("blur", close); };
  }, [open]);
  useEffect(() => { if (open) list.current?.querySelector(".rp-item.kb")?.scrollIntoView({ block: "nearest" }); }, [open, active]);
  useEffect(() => { if (disabled) close(); }, [disabled]);
  const onKeyDown = (e: React.KeyboardEvent) => {
    const down = e.key === "ArrowDown" || (vim && open && e.key === "j"), up = e.key === "ArrowUp" || (vim && open && e.key === "k");
    if (!open) { if (down || up || e.key === "Enter" || e.key === " ") { e.preventDefault(); e.stopPropagation(); show(); } return; }
    e.stopPropagation();
    if (e.key === "Escape" || e.key === "Tab") { if (e.key === "Escape") e.preventDefault(); close(); return; }
    e.preventDefault();
    if (down) { const next = enabled(active + 1, 1); if (next >= 0) setActive(next); }
    else if (up) { const next = enabled(active - 1, -1); if (next >= 0) setActive(next); }
    else if (e.key === "Home") setActive(enabled(0, 1));
    else if (e.key === "End") setActive(enabled(items.length - 1, -1));
    else if (e.key === "Enter" || e.key === " ") choose(items[active]);
  };
  return (
    <>
      <button ref={button} type="button" className={`rp-button${open ? " open" : ""}`} aria-label={tt("Repository")} aria-haspopup="listbox" aria-expanded={open} title={tt("Main repository or submodule")}
        disabled={disabled} onClick={() => (open ? close() : show())} onKeyDown={onKeyDown}>
        <Mark kind={current.mark} depth={0} /><span className="rp-name">{current.name}</span>{current.depth === 0 && <span className="rp-badge">{tt("main")}</span>}<span className="rp-caret" aria-hidden>▾</span>
      </button>
      {open && box && (
        <div ref={list} className="rp-menu" role="listbox" aria-label={tt("Repository")} style={{ ...box, width: undefined, minWidth: box.width, maxWidth: window.innerWidth - box.left - 8 }}>
          {items.map((item, index) => (
            <div key={item.path} role="option" aria-selected={item.path === value} aria-disabled={item.disabled} title={item.title}
              className={`rp-item${index === active ? " kb" : ""}${item.path === value ? " current" : ""}${item.disabled ? " disabled" : ""}`}
              onMouseEnter={(e) => { if (hover.current.moved(e) && !item.disabled) setActive(index); }} onMouseMove={(e) => { if (hover.current.moved(e) && !item.disabled) setActive(index); }} onClick={() => choose(item)}>
              {Array.from({ length: Math.max(0, item.depth) }, (_, level) => <span key={level} className={`rp-guide${level === item.depth - 1 ? " last" : ""}`} />)}
              <Mark kind={item.mark} depth={item.depth} />
              <span className="rp-name">{item.name}</span>
              {item.hint && <span className={`rp-badge${item.disabled ? " muted" : ""}`}>{item.hint}</span>}
              {item.path === value && <span className="rp-check" aria-hidden>✓</span>}
            </div>
          ))}
        </div>
      )}
    </>
  );
}
