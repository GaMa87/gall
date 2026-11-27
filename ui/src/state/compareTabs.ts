// The open comparisons of the comparison window, kept for the whole session and saved on every change, so that the
// window can show them as tabs and bring them back after a restart. The logic is in `core/compareTabs.ts`; this module
// holds the state, the saving (a moment after the last change, never while the file of a newer version is there),
// relinking of moved folders, and the one reading of what an earlier session left.
import { useSyncExternalStore } from "react";
import { api } from "../api/ipc";
import type { CompareSessionsLoaded, CompareTabCheck, CompareTabRecord, DiffSource } from "../api/types";
import { relinkFolders } from "../core/compareSession";
import { closeTab, fromRecord, openTab, tabKey, toSessions, type CompareTab, type CompareTabViewState } from "../core/compareTabs";

export interface EarlierTab {
  index: number;
  record: CompareTabRecord;
  check: CompareTabCheck;
  /** Non-null when both sides can be read again (`ready` or `changed`). */
  tab: CompareTab | null;
  label: string;
}

export interface CompareTabsState {
  tabs: CompareTab[];
  active: number;
  /** Tabs of an earlier session whose files can be read again and that are not open now. */
  earlier: CompareTab[];
  /** All tabs of an earlier session not open now, including missing, changed, and unsaved ones. */
  earlierAll: EarlierTab[];
}

let state: CompareTabsState = { tabs: [], active: 0, earlier: [], earlierAll: [] };
let version = 1;
let foreign = false;
let loading: Promise<void> | null = null;
let saveTimer: ReturnType<typeof setTimeout> | null = null;
const listeners = new Set<() => void>();

function emit() {
  listeners.forEach((l) => l());
}

function buildEarlier(loaded: CompareSessionsLoaded, openTabs: readonly CompareTab[]): { earlier: CompareTab[]; earlierAll: EarlierTab[] } {
  const open = new Set(openTabs.map((t) => tabKey(t.a, t.b)));
  const earlier: CompareTab[] = [];
  const earlierAll: EarlierTab[] = [];
  for (const check of loaded.checks) {
    const record = loaded.sessions.tabs[check.index];
    if (!record) continue;
    const candidate = fromRecord(record);
    if (candidate && open.has(tabKey(candidate.a, candidate.b))) continue;
    const openableTab = check.openable ? candidate : null;
    if (openableTab) earlier.push(openableTab);
    earlierAll.push({
      index: check.index,
      record,
      check,
      tab: openableTab,
      label: `${record.left.label} ↔ ${record.right.label}`,
    });
  }
  return { earlier, earlierAll };
}

function scheduleSave() {
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    // Only after what the last session left was read: the version to write is then known, and a file of a newer
    // version (written by a newer Gall) is never overwritten.
    void loadOnce().then(() => {
      if (!foreign) void api.compareSessionsSave(toSessions(state.tabs, state.active, version)).catch(() => {});
    });
  }, 500);
}

function set(next: CompareTabsState) {
  state = next;
  emit();
  scheduleSave();
}

/** Reads what the last session left, once. A file of a newer version is never overwritten. */
function loadOnce(): Promise<void> {
  loading ??= api.compareSessionsLoad().then((loaded) => {
    version = loaded.sessions.version;
    foreign = loaded.foreignVersion;
    const { earlier, earlierAll } = buildEarlier(loaded, state.tabs);
    state = { ...state, earlier, earlierAll };
    emit();
  }).catch(() => {});
  return loading;
}

/** A pair is shown in the window: it becomes (or is) a tab. */
export function registerPair(a: DiffSource, b: DiffSource, initial?: CompareTabViewState) {
  void loadOnce();
  const next = openTab(state.tabs, state.active, a, b, initial);
  const same = next.active === state.active && next.tabs.length === state.tabs.length
    && next.tabs.every((t, i) => t === state.tabs[i] || (t.a === state.tabs[i]?.a && t.b === state.tabs[i]?.b));
  if (same) return;
  const openKeys = new Set(next.tabs.map((t) => tabKey(t.a, t.b)));
  set({
    ...state,
    tabs: next.tabs,
    active: next.active,
    earlier: state.earlier.filter((e) => !openKeys.has(tabKey(e.a, e.b))),
    earlierAll: state.earlierAll.filter((e) => !(e.tab && openKeys.has(tabKey(e.tab.a, e.tab.b)))),
  });
}

/** Finds an open tab matching `(a, b)`, or `undefined` if not registered yet. */
export function findTab(a: DiffSource, b: DiffSource): CompareTab | undefined {
  const key = tabKey(a, b);
  return state.tabs.find((t) => tabKey(t.a, t.b) === key);
}

/** Persists a tab's view state (mode, diff options, logical scroll anchor, active hunk). */
export function updateTabState(a: DiffSource, b: DiffSource, patch: CompareTabViewState) {
  const key = tabKey(a, b);
  const idx = state.tabs.findIndex((t) => tabKey(t.a, t.b) === key);
  if (idx < 0) return;
  const cur = state.tabs[idx];
  const nextTab: CompareTab = {
    ...cur,
    ...(patch.mode !== undefined ? { mode: patch.mode } : {}),
    ...(patch.options !== undefined ? { options: patch.options } : {}),
    ...(patch.scroll !== undefined ? { scroll: patch.scroll } : {}),
    ...(patch.activeHunk !== undefined ? { activeHunk: patch.activeHunk } : {}),
  };
  if (
    cur.mode === nextTab.mode
    && cur.activeHunk === nextTab.activeHunk
    && JSON.stringify(cur.scroll ?? null) === JSON.stringify(nextTab.scroll ?? null)
    && JSON.stringify(cur.options ?? null) === JSON.stringify(nextTab.options ?? null)
  ) return;
  const tabs = state.tabs.slice();
  tabs[idx] = nextTab;
  set({ ...state, tabs });
}

/** Closes a tab; returns the tab that is shown next, or `null` when none is left. */
export function closeTabAt(index: number): CompareTab | null {
  const next = closeTab(state.tabs, index, state.active);
  set({ ...state, tabs: next.tabs, active: next.active ?? 0 });
  return next.active === null ? null : next.tabs[next.active];
}

/** Brings all openable tabs of an earlier session back; returns the one to show. */
export function restoreEarlier(): CompareTab | null {
  const bring = state.earlier;
  if (bring.length === 0) return null;
  let tabs = state.tabs, active = state.active;
  for (const t of bring) {
    ({ tabs, active } = openTab(tabs, active, t.a, t.b, {
      mode: t.mode,
      options: t.options,
      scroll: t.scroll,
      activeHunk: t.activeHunk,
    }));
  }
  const openKeys = new Set(tabs.map((t) => tabKey(t.a, t.b)));
  set({
    tabs,
    active,
    earlier: [],
    earlierAll: state.earlierAll.filter((e) => !(e.tab && openKeys.has(tabKey(e.tab.a, e.tab.b)))),
  });
  return tabs[active] ?? null;
}

/** Restores a single openable earlier tab (by its check index) and returns it. */
export function restoreEarlierAt(index: number): CompareTab | null {
  const entry = state.earlierAll.find((e) => e.index === index);
  if (!entry?.tab) return null;
  const t = entry.tab;
  const next = openTab(state.tabs, state.active, t.a, t.b, {
    mode: t.mode,
    options: t.options,
    scroll: t.scroll,
    activeHunk: t.activeHunk,
  });
  const openKeys = new Set(next.tabs.map((tab) => tabKey(tab.a, tab.b)));
  set({
    tabs: next.tabs,
    active: next.active,
    earlier: state.earlier.filter((e) => !openKeys.has(tabKey(e.a, e.b))),
    earlierAll: state.earlierAll.filter((e) => e.index !== index && !(e.tab && openKeys.has(tabKey(e.tab.a, e.tab.b)))),
  });
  return next.tabs[next.active] ?? null;
}

/** Dismisses the list of earlier tabs for this session without opening them. */
export function dismissEarlier() {
  if (state.earlier.length === 0 && state.earlierAll.length === 0) return;
  state = { ...state, earlier: [], earlierAll: [] };
  emit();
}

/**
 * Relinks earlier tabs whose files moved with their folder: computes the old/new folder pair
 * from `missingPath` and `chosenPath`, runs `compare_sessions_relink`, and refreshes the
 * earlier-tab checks. Returns the folder pair and how many tabs are now openable.
 */
export async function relinkEarlier(
  missingPath: string,
  chosenPath: string,
): Promise<{ pair: { from: string; to: string } | null; newlyOpenable: number; loaded: CompareSessionsLoaded | null }> {
  const pair = relinkFolders(missingPath, chosenPath);
  if (!pair) return { pair: null, newlyOpenable: 0, loaded: null };
  const beforeOpenable = state.earlier.length;
  const earlierSessions = state.earlierAll.length > 0
    ? { version, tabs: state.earlierAll.map((e) => e.record), active: 0 }
    : undefined;
  const loaded = await api.compareSessionsRelink(pair.from, pair.to, earlierSessions);
  version = loaded.sessions.version;
  foreign = loaded.foreignVersion;
  const { earlier, earlierAll } = buildEarlier(loaded, state.tabs);
  state = { ...state, earlier, earlierAll };
  emit();
  return { pair, newlyOpenable: Math.max(0, earlier.length - beforeOpenable), loaded };
}

/** The tab `delta` places from the shown one (wrapping round), or `null` when there is no other tab. */
export function neighbourTab(delta: number): CompareTab | null {
  const n = state.tabs.length;
  if (n < 2) return null;
  return state.tabs[(((state.active + delta) % n) + n) % n];
}

export function getCompareTabsState(): CompareTabsState {
  return state;
}

export function useCompareTabs(): CompareTabsState {
  return useSyncExternalStore((l) => { listeners.add(l); return () => { listeners.delete(l); }; }, () => state);
}

/** Flushes any pending debounced save immediately (used by tests / selftest before simulating restart). */
export async function flushCompareTabsSave(): Promise<void> {
  if (saveTimer) {
    clearTimeout(saveTimer);
    saveTimer = null;
  }
  await loadOnce();
  if (!foreign) await api.compareSessionsSave(toSessions(state.tabs, state.active, version));
}

/** Tests only: back to nothing open and nothing loaded. */
export function resetCompareTabs() {
  state = { tabs: [], active: 0, earlier: [], earlierAll: [] };
  version = 1; foreign = false; loading = null;
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = null;
  emit();
}
