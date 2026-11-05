// Trash window (sidebar › Trash): what is in the trash, where it came from, and
// Restore / Delete permanently / Empty trash. One window on every system: the Windows
// Recycle Bin is not a folder Gall can list, and Linux's Trash/files has no restore.
import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/ipc";
import type { TrashEntry } from "../api/types";
import { formatDate, formatSize, formatTime } from "../core/format";
import { join } from "../core/path";
import * as A from "../state/actions";
import { controller } from "../state/controllers";
import { getState, useStore } from "../state/store";
import { FileIcon, Icon } from "./Icon";
import { primaryMod } from "../core/keymap";

type SortKey = "name" | "originalParent" | "deleted" | "size";

const extOf = (name: string) => {
  const d = name.lastIndexOf(".");
  return d > 0 ? name.slice(d + 1).toLowerCase() : "";
};

export function TrashWindow() {
  const [items, setItems] = useState<TrashEntry[] | null>(null);
  const [filter, setFilter] = useState("");
  const [sort, setSort] = useState<{ key: SortKey; desc: boolean }>({ key: "deleted", desc: true });
  const [sel, setSel] = useState<Set<string>>(new Set());
  const [anchor, setAnchor] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const load = () => api.trashList().then((l) => {
    setItems(l);
    setSel((s) => new Set([...s].filter((id) => l.some((e) => e.id === id))));
  }).catch((e) => { A.fail(e); setItems([]); });

  const winRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    load();
    // Vim keys: the list has the keyboard (j/k…), / goes to the filter; else the filter.
    if (getState().settings?.vimKeys) winRef.current?.focus();
    else inputRef.current?.focus();
  }, []);

  const rows = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const list = (items ?? []).filter((e) => !q || e.name.toLowerCase().includes(q) || e.originalParent.toLowerCase().includes(q));
    const dir = sort.desc ? -1 : 1;
    return [...list].sort((a, b) => {
      const k = sort.key;
      const c = k === "deleted" || k === "size" ? a[k] - b[k] : a[k].localeCompare(b[k], undefined, { numeric: true, sensitivity: "base" });
      return c * dir || a.name.localeCompare(b.name);
    });
  }, [items, filter, sort]);

  const picked = rows.filter((e) => sel.has(e.id));
  const total = (items ?? []).reduce((n, e) => n + e.size, 0);
  const what = (list: TrashEntry[]) => (list.length === 1 ? `“${list[0].name}”` : A.items(list.length));
  const close = () => A.openOverlay(null);

  const click = (e: React.MouseEvent, i: number) => {
    const id = rows[i].id;
    if (e.shiftKey && anchor != null) {
      const [a, b] = [Math.min(anchor, i), Math.max(anchor, i)];
      setSel(new Set(rows.slice(a, b + 1).map((r) => r.id)));
      return;
    }
    setAnchor(i);
    setSel((s) => {
      const n = new Set(e.ctrlKey || e.metaKey ? s : []);
      if (n.has(id) && (e.ctrlKey || e.metaKey)) n.delete(id);
      else n.add(id);
      return n;
    });
  };

  const run = async (f: () => Promise<void>) => {
    setBusy(true);
    try {
      await f();
    } catch (e) {
      A.fail(e);
    } finally {
      setBusy(false);
      load();
    }
  };

  const restore = (list: TrashEntry[]) => list.length && run(async () => {
    await api.trashRestore(list.map((e) => e.id));
    const first = list[0];
    const path = join(first.originalParent, first.name);
    A.notify(`Restored ${what(list)}`, "success", {
      label: "Show",
      run: () => {
        close();
        const b = A.activeBrowser();
        A.navigate(b, first.originalParent);
        setTimeout(() => controller(b)?.select([path], path), 200);
      },
    });
  });

  const purge = async (list: TrashEntry[]) => {
    if (!list.length) return;
    if (!(await A.confirm("Delete permanently?", `${what(list)} will be deleted and cannot be restored.`, "Delete", true))) return;
    run(async () => {
      await api.trashPurge(list.map((e) => e.id));
      A.notify(`Deleted ${what(list)} permanently`, "info");
    });
  };

  const empty = async () => {
    if (!items?.length) return;
    if (!(await A.confirm("Empty the trash?", `${A.items(items.length)} (${formatSize(total)}) will be deleted and cannot be restored.`, "Empty trash", true))) return;
    run(async () => {
      await api.trashPurge([]);
      A.notify("Emptied the trash", "info");
    });
  };

  const gAt = useRef(0);
  const vimKeys = useStore((st) => st.settings?.vimKeys ?? false);
  const goTo = (i: number) => {
    if (!rows[i]) return;
    setAnchor(i);
    setSel(new Set([rows[i].id]));
    document.querySelector(`.trash-win .r[data-i="${i}"]`)?.scrollIntoView({ block: "nearest" });
  };

  const onKey = (e: React.KeyboardEvent) => {
    e.stopPropagation();
    const typing = (e.target as HTMLElement).tagName === "INPUT";
    // Vim keys: Esc in the filter goes back to the list (a second Esc closes).
    if (e.key === "Escape" && typing && vimKeys) return winRef.current?.focus();
    if (e.key === "Escape") return close();
    // Vim keys (outside the filter field): j/k move, gg/G ends, q closes.
    if (vimKeys && !typing && !e.ctrlKey && !e.altKey && !e.metaKey) {
      const k = e.key;
      if (k === "q") return close();
      if (k === "/") { e.preventDefault(); return inputRef.current?.focus(); }
      if (k === "G") { e.preventDefault(); return goTo(rows.length - 1); }
      if (k === "g") {
        e.preventDefault();
        if (Date.now() - gAt.current < 600) { gAt.current = 0; goTo(0); } else gAt.current = Date.now();
        return;
      }
      if (k === "j" || k === "k") {
        e.preventDefault();
        return goTo(Math.max(0, Math.min(rows.length - 1, (anchor ?? -1) + (k === "j" ? 1 : -1))));
      }
    }
    if (e.key === "a" && primaryMod(e) && !typing) {
      e.preventDefault();
      setSel(new Set(rows.map((r) => r.id)));
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      goTo(Math.max(0, Math.min(rows.length - 1, (anchor ?? -1) + (e.key === "ArrowDown" ? 1 : -1))));
    } else if (e.key === "Enter" && picked.length) {
      e.preventDefault();
      restore(picked);
    } else if (e.key === "Delete" && picked.length && !(typing && filter)) {
      e.preventDefault();
      purge(picked);
    }
  };

  const head = (key: SortKey, label: string, cls = "") => (
    <span className={`h ${cls} ${sort.key === key ? "on" : ""}`} onClick={() => setSort((s) => ({ key, desc: s.key === key ? !s.desc : key === "deleted" || key === "size" }))}>
      {label}{sort.key === key ? (sort.desc ? " ↓" : " ↑") : ""}
    </span>
  );

  return (
    <div className="backdrop" onMouseDown={close}>
      <div ref={winRef} className="modal search-win trash-win" onMouseDown={(e) => e.stopPropagation()} tabIndex={-1} onKeyDown={onKey}>
        <div className="shead">
          <Icon name="trash" size={17} />
          <b>Trash</b>
          <span className="tw-sum">{items == null ? "" : items.length ? `${A.items(items.length)} · ${formatSize(total)}` : "empty"}</span>
          <input ref={inputRef} type="text" placeholder="Filter by name or location" value={filter} onChange={(e) => setFilter(e.target.value)} />
          <button className="icon-btn" title="Reload" onClick={() => load()}><Icon name="refresh" size={15} /></button>
        </div>
        <div className="r tw-head">
          <span />
          {head("name", "Name")}
          {head("originalParent", "Original location")}
          {head("deleted", "Deleted")}
          {head("size", "Size", "num")}
        </div>
        <div className="results">
          {items == null && <div className="tw-empty">Reading the trash…</div>}
          {items != null && !items.length && <div className="tw-empty"><Icon name="trash" size={40} /><span>The trash is empty</span></div>}
          {items != null && items.length > 0 && !rows.length && <div className="tw-empty">Nothing matches “{filter}”</div>}
          {rows.map((r, i) => {
            const ms = r.deleted * 1000;
            return (
              <div key={r.id} data-i={i} className={`r ${sel.has(r.id) ? "on" : ""}`} onClick={(e) => click(e, i)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  const list = sel.has(r.id) ? picked : [r];
                  if (!sel.has(r.id)) { setSel(new Set([r.id])); setAnchor(i); }
                  A.showMenu(e.clientX, e.clientY, [
                    { label: list.length > 1 ? `Restore ${list.length} items` : "Restore", run: () => restore(list) },
                    { label: "Open original folder", run: () => { close(); A.navigate(A.activeBrowser(), r.originalParent); } },
                    { separator: true },
                    { label: "Delete permanently…", danger: true, run: () => purge(list) },
                  ]);
                }}>
                <FileIcon entry={{ isDir: r.isDir, ext: r.isDir ? "" : extOf(r.name), name: r.name }} />
                <span className="n" title={r.name}>{r.name}</span>
                <span className="p" title={r.originalParent}><bdi>{r.originalParent}</bdi></span>
                <span className="d">{formatDate(ms)} {formatTime(ms)}</span>
                <span className="num">{formatSize(r.size)}</span>
              </div>
            );
          })}
        </div>
        <div className="foot tw-foot">
          <button className="btn primary" disabled={!picked.length || busy} onClick={() => restore(picked)}>
            Restore{picked.length > 1 ? ` ${picked.length} items` : ""}
          </button>
          <button className="btn" disabled={!picked.length || busy} onClick={() => purge(picked)}>Delete permanently</button>
          <span className="tw-hint">Enter restore · Del delete · Ctrl+A all</span>
          <button className="btn danger" disabled={!items?.length || busy} onClick={empty}>Empty trash</button>
        </div>
      </div>
    </div>
  );
}
