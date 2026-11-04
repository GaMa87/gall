// Browser tabs (T-01…T-10).
import { useEffect, useRef, useState } from "react";
import type { BrowserState } from "../api/types";
import { basename } from "../core/path";
import * as A from "../state/actions";
import { draggedTab, dropInto, isFileDrag, onDragOverTarget, startTabDrag } from "../state/dnd";
import { useStore } from "../state/store";
import { Icon } from "./Icon";


export function TabBar({ b, browser, right }: { b: number; browser: BrowserState; right?: React.ReactNode }) {
  const settings = useStore((s) => s.settings)!;
  const [armed, setArmed] = useState<number | null>(null);
  const [drop, setDrop] = useState<string | null>(null);
  const armTimer = useRef<number | null>(null);
  useEffect(() => () => { if (armTimer.current) clearTimeout(armTimer.current); }, []);

  const tabMenu = (e: React.MouseEvent, idx: number) => {
    e.preventDefault();
    const t = browser.tabs[idx];
    A.showMenu(e.clientX, e.clientY, [
      { label: "New tab", shortcut: "Ctrl+T", run: () => A.sessionCmd({ op: "newTab", b }) },
      { label: "Duplicate to other browser", run: () => A.sessionCmd({ op: "transfer", b, idx, copy: true }) },
      { label: "Move to other browser", shortcut: "Alt+Middle-click", run: () => A.sessionCmd({ op: "transfer", b, idx, copy: false }) },
      { separator: true },
      {
        label: "Change alias…",
        run: async () => {
          const v = await A.promptText("Tab alias", t.alias ?? basename(t.path), "Save");
          if (v !== null) A.sessionCmd({ op: "alias", b, idx, alias: v, keep: t.keepAlias });
        },
      },
      { label: "Keep alias when navigating", checked: t.keepAlias, disabled: !t.alias, run: () => A.sessionCmd({ op: "alias", b, idx, alias: t.alias, keep: !t.keepAlias }) },
      { label: "Remove alias", disabled: !t.alias, run: () => A.sessionCmd({ op: "alias", b, idx, alias: null, keep: false }) },
      { separator: true },
      { label: "Close", shortcut: "Ctrl+W", disabled: browser.tabs.length < 2, run: () => A.sessionCmd({ op: "close", b, idx }) },
      { label: "Close other tabs", disabled: browser.tabs.length < 2, run: () => A.sessionCmd({ op: "closeOthers", b, idx }) },
      { label: "Close stale tabs (unused 24 h)", run: () => A.sessionCmd({ op: "closeStale", b }) },
      { label: "Reopen closed tab", shortcut: "Ctrl+Shift+T", disabled: !browser.closed.length, run: () => A.sessionCmd({ op: "reopen", b }) },
    ]);
  };

  return (
    <div
      className="tabbar"
      onWheel={(e) => {
        const n = browser.tabs.length;
        if (n < 2) return;
        const next = (browser.active + (e.deltaY > 0 ? 1 : -1) + n) % n;
        A.sessionCmd({ op: "activate", b, idx: next });
      }}
    >
      {browser.tabs.map((t, idx) => {
        const label = t.alias ?? (settings.tabsShowFullPath ? t.path : basename(t.path));
        const k = `t${t.id}`;
        return (
          <div
            key={t.id}
            className={`tab ${idx === browser.active ? "active" : ""} ${t.alias ? "alias" : ""} ${drop === k ? "drop" : ""}`}
            title={t.path}
            draggable
            onDragStart={(e) => startTabDrag(e, b, idx, t.path)}
            onMouseEnter={() => {
              armTimer.current = window.setTimeout(() => setArmed(t.id), 500);
            }}
            onMouseLeave={() => {
              if (armTimer.current) clearTimeout(armTimer.current);
              setArmed(null);
            }}
            onMouseDown={(e) => {
              if (e.button === 1) {
                e.preventDefault();
                if (e.altKey) A.sessionCmd({ op: "transfer", b, idx, copy: false });
                else A.sessionCmd({ op: "close", b, idx });
              }
            }}
            onClick={() => {
              A.focusBrowser(b);
              A.sessionCmd({ op: "activate", b, idx });
            }}
            onContextMenu={(e) => tabMenu(e, idx)}
            onDragOver={(e) => {
              if (draggedTab()) {
                e.preventDefault();
                setDrop(k);
              } else if (isFileDrag(e) && onDragOverTarget(e, t.path)) setDrop(k);
            }}
            onDragLeave={() => setDrop(null)}
            onDrop={(e) => {
              setDrop(null);
              const src = draggedTab();
              if (src) {
                e.preventDefault();
                if (src.b === b) A.sessionCmd({ op: "reorder", b, from: src.idx, to: idx });
                else A.sessionCmd({ op: "transfer", b: src.b, idx: src.idx, copy: false });
                return;
              }
              dropInto(e, t.path);
            }}
          >
            <Icon name="folder" size={13} color="var(--folder)" />
            <span className="label">{label}</span>
            {browser.tabs.length > 1 && (
              <button
                className={`close ${armed === t.id || idx === browser.active ? "armed" : ""}`}
                title="Close tab (Ctrl+W)"
                onClick={(e) => {
                  e.stopPropagation();
                  A.sessionCmd({ op: "close", b, idx });
                }}
              >
                ×
              </button>
            )}
          </div>
        );
      })}
      <button
        className={`tab-add ${drop === "add" ? "drop" : ""}`}
        title="New tab (Ctrl+T) — drop a tab here to duplicate it"
        onClick={() => A.sessionCmd({ op: "newTab", b })}
        onDragOver={(e) => {
          if (draggedTab()) {
            e.preventDefault();
            setDrop("add");
          }
        }}
        onDragLeave={() => setDrop(null)}
        onDrop={(e) => {
          setDrop(null);
          const src = draggedTab();
          if (!src) return;
          e.preventDefault();
          if (src.b === b) A.sessionCmd({ op: "newTab", b, path: browserPath(src) });
          else A.sessionCmd({ op: "transfer", b: src.b, idx: src.idx, copy: true });
        }}
      >
        <Icon name="plus" size={14} />
      </button>
      <div style={{ flex: 1 }} />
      {right}
    </div>
  );

  function browserPath(src: { idx: number }) {
    return browser.tabs[src.idx]?.path;
  }
}
