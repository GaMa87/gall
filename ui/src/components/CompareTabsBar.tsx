import { useEffect, useState } from "react";
import type { CompareSourceState, DiffSource } from "../api/types";
import type { Overlay } from "../state/store";
import * as A from "../state/actions";
import {
  closeTabAt,
  dismissEarlier,
  registerPair,
  relinkEarlier,
  restoreEarlier,
  restoreEarlierAt,
  useCompareTabs,
  type EarlierTab,
} from "../state/compareTabs";
import { tt } from "../i18n";
import type { CompareTab } from "../core/compareTabs";

function missingFilePath(entry: EarlierTab): string | null {
  if (entry.check.left === "missing" && entry.record.left.source?.kind === "file") {
    return entry.record.left.source.path;
  }
  if (entry.check.right === "missing" && entry.record.right.source?.kind === "file") {
    return entry.record.right.source.path;
  }
  return null;
}

function stateBadge(state: CompareSourceState): { cls: string; text: string } {
  switch (state) {
    case "missing": return { cls: "missing", text: tt("Missing") };
    case "changed": return { cls: "changed", text: tt("Changed") };
    case "notSaved": return { cls: "notSaved", text: tt("Not saved") };
    case "ready": return { cls: "ready", text: tt("Ready") };
  }
}

/**
 * The open comparisons as tabs above the comparison window. The pair that is shown is registered as a tab (and a pair that
 * is already open is shown again, not added twice); the tabs are saved as they change and an earlier session's tabs
 * (including missing, changed, and unsaved checks) can be restored or relinked. Nothing is drawn while there is one
 * comparison and nothing earlier.
 * Arrow keys move between the tabs, Delete closes the focused one.
 */
export function CompareTabsBar({ a, b, back, beforeNavigate }: {
  a: DiffSource; b: DiffSource; back?: Overlay; beforeNavigate: () => Promise<boolean>;
}) {
  const { tabs, active, earlier, earlierAll } = useCompareTabs();
  const [showEarlier, setShowEarlier] = useState(false);
  const [relinking, setRelinking] = useState(false);
  const key = JSON.stringify([a, b]);
  useEffect(() => { registerPair(a, b); }, [key]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => { if (earlierAll.length === 0 && showEarlier) setShowEarlier(false); }, [earlierAll.length, showEarlier]);

  const present = (tab: CompareTab | null) => A.openOverlay(tab ? { type: "diff", a: tab.a, b: tab.b, back } : null);
  const show = async (tab: CompareTab) => {
    if (await beforeNavigate()) present(tab);
  };
  const closeAt = async (index: number) => {
    const wasActive = index === active;
    if (wasActive && !(await beforeNavigate())) return;
    const next = closeTabAt(index);
    if (wasActive) present(next);
  };
  const restore = async (getTab: () => CompareTab | null) => {
    if (!(await beforeNavigate())) return;
    const next = getTab();
    if (next) present(next);
  };

  const locateMissing = async (entry: EarlierTab, chosenOverride?: string) => {
    const missing = missingFilePath(entry);
    if (!missing || relinking) return;
    let chosen = chosenOverride;
    if (!chosen) {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({
        title: tt("Locate moved file: {name}", { name: entry.record.left.source?.kind === "file" && entry.check.left === "missing" ? entry.record.left.label : entry.record.right.label }),
        multiple: false,
        directory: false,
      });
      if (!picked || Array.isArray(picked)) return;
      chosen = picked;
    }
    setRelinking(true);
    try {
      const result = await relinkEarlier(missing, chosen);
      if (!result.pair) {
        A.notify(tt("Could not match a moved folder from that file (choose the same file name in its new folder)."), "warn");
        return;
      }
      if (result.newlyOpenable > 0) {
        A.notify(
          tt(result.newlyOpenable === 1 ? "Relinked {count} earlier comparison" : "Relinked {count} earlier comparisons", { count: result.newlyOpenable }),
          "success",
        );
      } else {
        A.notify(tt("Updated earlier paths, though some files are still missing."), "info");
      }
    } catch (e) {
      A.fail(e);
    } finally {
      setRelinking(false);
    }
  };

  if (tabs.length <= 1 && earlierAll.length === 0) return null;

  const missingCount = earlierAll.filter((e) => e.check.left === "missing" || e.check.right === "missing").length;
  const hasChecks = earlierAll.some((e) => e.check.left !== "ready" || e.check.right !== "ready");

  return (
    <div className="cmp-tabs-wrap" onMouseDown={(e) => e.stopPropagation()}>
      <div className="cmp-tabs" role="tablist" aria-label={tt("Comparison tabs")}
        onKeyDown={(e) => {
          const at = tabs.findIndex((_, i) => i === active);
          if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
            e.preventDefault(); e.stopPropagation();
            const next = tabs[(at + (e.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length];
            if (next) void show(next);
          } else if (e.key === "Delete" && (e.target as HTMLElement).closest("[role=tab]")) {
            e.preventDefault(); e.stopPropagation();
            void closeAt(at);
          }
        }}>
        {tabs.map((tab, i) => (
          <span key={`${i}:${tab.label}`} className={`cmp-tab ${i === active ? "on" : ""}`}>
            <button role="tab" aria-selected={i === active} tabIndex={i === active ? 0 : -1} title={tab.label}
              onClick={() => { if (i !== active) void show(tab); }}>{tab.label}</button>
            <button className="cmp-tab-x" aria-label={tt("Close comparison tab")} title={tt("Close comparison tab")}
              onClick={(e) => { e.stopPropagation(); void closeAt(i); }}>×</button>
          </span>
        ))}
        {earlier.length > 0 && (
          <button className="cmp-tabs-restore" onClick={() => { void restore(restoreEarlier); }}>
            {tt("Restore {count} earlier comparisons", { count: earlier.length })}
          </button>
        )}
        {earlierAll.length > 0 && (hasChecks || earlier.length === 0) && (
          <button
            className={`cmp-tabs-checks ${showEarlier ? "on" : ""}`}
            aria-expanded={showEarlier}
            onClick={() => setShowEarlier((v) => !v)}
          >
            {missingCount > 0
              ? tt(missingCount === 1 ? "{count} missing earlier tab" : "{count} missing earlier tabs", { count: missingCount })
              : tt("Earlier session ({count})", { count: earlierAll.length })}
          </button>
        )}
      </div>
      {showEarlier && earlierAll.length > 0 && (
        <div className="cmp-earlier-panel" role="region" aria-label={tt("Earlier comparison tabs")}>
          <div className="cmp-earlier-head">
            <span>{tt("Earlier comparison tabs")}</span>
            <button className="cmp-earlier-dismiss" onClick={() => { dismissEarlier(); setShowEarlier(false); }}>
              {tt("Dismiss")}
            </button>
          </div>
          <ul className="cmp-earlier-list">
            {earlierAll.map((entry) => {
              const leftBadge = stateBadge(entry.check.left);
              const rightBadge = stateBadge(entry.check.right);
              const missingPath = missingFilePath(entry);
              return (
                <li key={entry.index} className="cmp-earlier-row" data-earlier-index={entry.index}>
                  <span className="cmp-earlier-label" title={entry.label}>{entry.label}</span>
                  <span className="cmp-earlier-badges">
                    <span className={`cmp-check ${leftBadge.cls}`} title={entry.record.left.source?.kind === "file" ? entry.record.left.source.path : entry.record.left.label}>
                      {tt("Left")}: {leftBadge.text}
                    </span>
                    <span className={`cmp-check ${rightBadge.cls}`} title={entry.record.right.source?.kind === "file" ? entry.record.right.source.path : entry.record.right.label}>
                      {tt("Right")}: {rightBadge.text}
                    </span>
                  </span>
                  <span className="cmp-earlier-actions">
                    {entry.tab && (
                      <button className="dc-btn" onClick={() => { void restore(() => restoreEarlierAt(entry.index)); }}>
                        {tt("Open")}
                      </button>
                    )}
                    {missingPath && (
                      <button className="dc-btn" disabled={relinking} onClick={() => void locateMissing(entry)}>
                        {tt("Locate…")}
                      </button>
                    )}
                  </span>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </div>
  );
}
