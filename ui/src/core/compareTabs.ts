// The open comparisons of the comparison window as tabs: which pairs are open, which is shown, and how they are saved
// and restored. Pure, so it is unit-tested; the store and the tab strip only call it.
import type { CompareSessions, CompareTabRecord, DiffOptions, DiffSource } from "../api/types";
import type { ScrollAnchor } from "./compareSession";
import { basename } from "./path";

export interface CompareTab {
  a: DiffSource;
  b: DiffSource;
  label: string;
  mode?: "split" | "unified";
  options?: Partial<DiffOptions>;
  scroll?: ScrollAnchor | null;
  activeHunk?: number | null;
}

export interface CompareTabViewState {
  mode?: "split" | "unified";
  options?: Partial<DiffOptions>;
  scroll?: ScrollAnchor | null;
  activeHunk?: number | null;
}

/** Most tabs open at once; opening another closes the oldest one that is not shown. */
export const MAX_TABS = 12;

/** What a side is called in a tab. */
export function sourceLabel(source: DiffSource): string {
  switch (source.kind) {
    case "file": return basename(source.path);
    case "gitHead": return `${basename(source.path)} (HEAD)`;
    case "gitCommit": return `${basename(source.path)} (${source.commit.slice(0, 7)})`;
    case "gitBlob": return `${basename(source.file)} (${source.commit.slice(0, 7)})`;
    case "text": return source.label;
  }
}

export const tabLabel = (a: DiffSource, b: DiffSource) => `${sourceLabel(a)} ↔ ${sourceLabel(b)}`;
export const tabKey = (a: DiffSource, b: DiffSource) => JSON.stringify([a, b]);

/** Opens a pair: a pair that is already open is shown, else it is added after the shown tab and becomes the shown one. */
export function openTab(tabs: readonly CompareTab[], active: number, a: DiffSource, b: DiffSource, initial?: CompareTabViewState): { tabs: CompareTab[]; active: number } {
  const key = tabKey(a, b);
  const existing = tabs.findIndex((t) => tabKey(t.a, t.b) === key);
  if (existing >= 0) return { tabs: [...tabs], active: existing };
  const next = [...tabs];
  const at = tabs.length === 0 ? 0 : Math.min(active + 1, tabs.length);
  next.splice(at, 0, { a, b, label: tabLabel(a, b), ...initial });
  let shown = at;
  if (next.length > MAX_TABS) {
    // The oldest tab that is not the new one goes.
    const drop = next.findIndex((_, i) => i !== at);
    next.splice(drop, 1);
    if (drop < shown) shown--;
  }
  return { tabs: next, active: shown };
}

/** Closes a tab. The shown tab after it is the neighbour (the next one, else the previous); `null` when none is left. */
export function closeTab(tabs: readonly CompareTab[], index: number, active: number): { tabs: CompareTab[]; active: number | null } {
  if (index < 0 || index >= tabs.length) return { tabs: [...tabs], active };
  const next = tabs.filter((_, i) => i !== index);
  if (next.length === 0) return { tabs: next, active: null };
  if (index < active) return { tabs: next, active: active - 1 };
  if (index > active) return { tabs: next, active };
  return { tabs: next, active: Math.min(index, next.length - 1) };
}

/** The saved form of the open tabs. Text that was typed or pasted is not kept: its side has no source (only its label). */
export function toSessions(tabs: readonly CompareTab[], active: number, version: number): CompareSessions {
  const record = (t: CompareTab): CompareTabRecord => ({
    left: { source: t.a.kind === "text" ? null : t.a, label: sourceLabel(t.a) },
    right: { source: t.b.kind === "text" ? null : t.b, label: sourceLabel(t.b) },
    mode: t.mode ?? "split",
    options: t.options ?? {},
    scroll: t.scroll ?? null,
    activeHunk: t.activeHunk ?? null,
  });
  return { version, tabs: tabs.map(record), active: Math.max(0, Math.min(active, tabs.length - 1)) };
}

/** A saved tab as an open one, or `null` when a side was not kept (nothing to read again). */
export function fromRecord(record: CompareTabRecord): CompareTab | null {
  const a = record.left.source, b = record.right.source;
  if (!a || !b) return null;
  return {
    a,
    b,
    label: tabLabel(a, b),
    mode: record.mode,
    options: record.options,
    scroll: record.scroll ?? null,
    activeHunk: record.activeHunk ?? null,
  };
}
