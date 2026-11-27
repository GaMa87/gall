// Data hooks: folder listings with a shared cache, auto-refresh from the
// backend watcher, and per-folder preferences.
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "../api/ipc";
import type { FileEntry, GitStatus, HiddenState, ResolvedPrefs, Settings } from "../api/types";
import { archiveOf } from "../core/archive";
import { createRafCoalescer } from "../core/resizeThrottle";
import { onFolderChanged } from "./controllers";
import { useStore } from "./store";

interface CacheEntry {
  entries: FileEntry[];
  at: number;
}
const cache = new Map<string, CacheEntry>();
const key = (p: string, dirsOnly: boolean, hidden: boolean, temp: boolean) => `${dirsOnly ? "d" : "a"}${hidden ? "h" : ""}${temp ? "t" : ""}|${p}`;

export function invalidate(path: string) {
  for (const k of [...cache.keys()]) if (k.endsWith("|" + path)) cache.delete(k);
}

export interface DirState {
  entries: FileEntry[];
  loading: boolean;
  error: string | null;
  reload: () => void;
}

/**
 * Lists a folder and keeps it fresh. While the folder is displayed it is
 * watched by the backend; changes trigger a (throttled during storms) reload.
 */
export function useDirList(path: string | null, settings: Settings | null, dirsOnly = false, showHiddenOverride?: boolean): DirState {
  const showHidden = showHiddenOverride ?? settings?.showHidden ?? false;
  const hideTemp = settings?.hideTemp ?? false;
  const k = path ? key(path, dirsOnly, showHidden, hideTemp) : "";
  const [state, setState] = useState<{ entries: FileEntry[]; error: string | null; loading: boolean; k: string }>(() => ({
    entries: cache.get(k)?.entries ?? [], error: null, loading: !cache.has(k), k,
  }));
  const seq = useRef(0);
  const lastStormReload = useRef(0);

  const load = () => {
    if (!path) return;
    const my = ++seq.current;
    api
      .listDir(path, { showHidden, hideTemp, dirsOnly })
      .then((entries) => {
        if (my !== seq.current) return;
        cache.set(k, { entries, at: Date.now() });
        setState({ entries, error: null, loading: false, k });
      })
      .catch((err) => {
        if (my !== seq.current) return;
        setState({ entries: [], error: String(err), loading: false, k });
      });
  };

  useEffect(() => {
    if (!path) return;
    const cached = cache.get(k);
    setState({ entries: cached?.entries ?? [], error: null, loading: !cached, k });
    load();
    api.watch(path).catch(() => {});
    const off = onFolderChanged(path, (storm) => {
      if (storm) {
        const now = Date.now();
        if (now - lastStormReload.current < 2000) return;
        lastStormReload.current = now;
      }
      load();
    });
    return () => {
      off();
      api.unwatch(path).catch(() => {});
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [k]);

  const current = state.k === k ? state : { entries: cache.get(k)?.entries ?? [], error: null, loading: true };
  return { entries: current.entries, loading: current.loading, error: current.error, reload: load };
}

export function usePrefs(path: string | null, nonce: number): [ResolvedPrefs | null, () => void] {
  const [prefs, setPrefs] = useState<ResolvedPrefs | null>(null);
  const [tick, setTick] = useState(0);
  // Default sort / view come from the settings: resolve again when they change.
  const settingsRev = useStore((s) => s.settingsRev);
  useEffect(() => {
    if (!path) return;
    let alive = true;
    api.resolvePrefs(path).then((p) => alive && setPrefs(p)).catch(() => {});
    return () => {
      alive = false;
    };
  }, [path, nonce, tick, settingsRev]);
  return [prefs, () => setTick((t) => t + 1)];
}

export function useTags(path: string | null, nonce: number): Map<string, number> {
  const [tags, setTags] = useState<Map<string, number>>(new Map());
  useEffect(() => {
    if (!path) return;
    let alive = true;
    api.tagsIn(path).then((t) => alive && setTags(new Map(t.map((x) => [x.path, x.color])))).catch(() => {});
    return () => {
      alive = false;
    };
  }, [path, nonce]);
  return tags;
}

export interface ElementSize {
  w: number;
  h: number;
  scrollbarW: number;
}

/**
 * Observes an element's size, coalesced to one update per animation frame and deduplicated
 * on whole CSS pixels (plus vertical scrollbar width so callers avoid forced layout in render).
 * Optional `shouldUpdate` lets list views ignore sub-threshold pixel changes; `syncKey` forces
 * an exact re-measure when switching modes (e.g. list → grid).
 */
export function useSize<T extends HTMLElement>(
  shouldUpdate?: (prev: ElementSize, next: ElementSize) => boolean,
  syncKey?: unknown,
): [React.RefObject<T | null>, ElementSize] {
  const ref = useRef<T | null>(null);
  const observed = useRef<T | null>(null);
  const ro = useRef<ResizeObserver | null>(null);
  const raf = useRef<ReturnType<typeof createRafCoalescer> | null>(null);
  const shouldUpdateRef = useRef(shouldUpdate);
  shouldUpdateRef.current = shouldUpdate;
  const lastSyncKey = useRef(syncKey);
  const [size, setSize] = useState<ElementSize>({ w: 0, h: 0, scrollbarW: 0 });
  // Re-check after every render: the element may appear later or be replaced.
  useLayoutEffect(() => {
    const el = ref.current;
    const syncChanged = lastSyncKey.current !== syncKey;
    lastSyncKey.current = syncKey;
    if (el === observed.current) {
      if (el && syncChanged) {
        const next = { w: Math.round(el.clientWidth), h: Math.round(el.clientHeight), scrollbarW: Math.max(0, el.offsetWidth - el.clientWidth) };
        setSize((prev) => (prev.w === next.w && prev.h === next.h && prev.scrollbarW === next.scrollbarW ? prev : next));
      }
      return;
    }
    ro.current?.disconnect();
    raf.current?.cancel();
    observed.current = el;
    if (!el) return;
    const measure = (forceExact = false) => {
      const next: ElementSize = {
        w: Math.round(el.clientWidth),
        h: Math.round(el.clientHeight),
        scrollbarW: Math.max(0, el.offsetWidth - el.clientWidth),
      };
      setSize((prev) => {
        if (prev.w === next.w && prev.h === next.h && prev.scrollbarW === next.scrollbarW) return prev;
        if (!forceExact && shouldUpdateRef.current && !shouldUpdateRef.current(prev, next)) return prev;
        return next;
      });
    };
    const coalescer = createRafCoalescer(() => measure(false));
    raf.current = coalescer;
    ro.current = new ResizeObserver(() => coalescer.schedule());
    ro.current.observe(el);
    measure(true);
  });
  useEffect(
    () => () => {
      ro.current?.disconnect();
      raf.current?.cancel();
      // Forget the element too, so a re-run (React StrictMode remount) observes it again
      // instead of keeping the stale first-render size forever.
      observed.current = null;
    },
    [],
  );
  return [ref, size];
}

/**
 * Observes whether an element's width exceeds `threshold`, only updating React state
 * when that boolean flips (avoiding per-pixel re-renders during live window resizes).
 */
export function useWidthAbove<T extends HTMLElement>(threshold: number): [React.RefObject<T | null>, boolean] {
  const ref = useRef<T | null>(null);
  const observed = useRef<T | null>(null);
  const ro = useRef<ResizeObserver | null>(null);
  const raf = useRef<ReturnType<typeof createRafCoalescer> | null>(null);
  const thresholdRef = useRef(threshold);
  thresholdRef.current = threshold;
  const [above, setAbove] = useState(false);

  useLayoutEffect(() => {
    const el = ref.current;
    if (el === observed.current) {
      if (el) {
        const next = el.clientWidth > thresholdRef.current;
        setAbove((prev) => (prev === next ? prev : next));
      }
      return;
    }
    ro.current?.disconnect();
    raf.current?.cancel();
    observed.current = el;
    if (!el) return;
    const check = () => {
      const next = el.clientWidth > thresholdRef.current;
      setAbove((prev) => (prev === next ? prev : next));
    };
    const coalescer = createRafCoalescer(check);
    raf.current = coalescer;
    ro.current = new ResizeObserver(() => coalescer.schedule());
    ro.current.observe(el);
    check();
  });

  useEffect(
    () => () => {
      ro.current?.disconnect();
      raf.current?.cancel();
      observed.current = null;
    },
    [],
  );
  return [ref, above];
}

/** Global tick used to refresh tags/prefs after edits. */
let nonce = 0;
const nonceSubs = new Set<() => void>();
export function bumpNonce() {
  nonce++;
  nonceSubs.forEach((f) => f());
}
export function useNonce(): number {
  const [n, setN] = useState(nonce);
  useEffect(() => {
    const f = () => setN(nonce);
    nonceSubs.add(f);
    return () => {
      nonceSubs.delete(f);
    };
  }, []);
  return n;
}

/**
 * Git status of the items in `path` (badges) and the branch; null outside a repository
 * or when off. Refreshed when the folder changes and when the window gets the focus
 * back (after a commit in a terminal); one request at a time.
 */
const gitSubs = new Set<() => void>();
/** Reloads every folder's Git status and trust state (after a refused repository was trusted). */
export function bumpGitStatus() {
  gitSubs.forEach((f) => f());
}

/**
 * The repository path when Git refuses the folder's repository as owned by another user
 * ("dubious ownership"), which hides the branch and the badges; null otherwise. Only asked
 * while there is no status (`hasGit` false), so a working repository costs nothing.
 */
export function useGitUntrusted(path: string, enabled: boolean, hasGit: boolean): string | null {
  const [state, setState] = useState<{ path: string; repo: string | null } | null>(null);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const again = () => setTick((n) => n + 1);
    gitSubs.add(again);
    return () => { gitSubs.delete(again); };
  }, []);
  useEffect(() => {
    if (!enabled || !path || hasGit || archiveOf(path)) { setState(null); return; }
    let alive = true;
    api.gitUntrusted(path)
      .then((repo) => alive && setState({ path, repo }))
      .catch(() => alive && setState({ path, repo: null }));
    return () => { alive = false; };
  }, [path, enabled, hasGit, tick]);
  return state && state.path === path ? state.repo : null;
}

export function useGitStatus(path: string, enabled: boolean): GitStatus | null {
  const [st, setSt] = useState<{ path: string; st: GitStatus | null } | null>(null);
  useEffect(() => {
    if (!enabled || !path || archiveOf(path)) {
      setSt(null);
      return;
    }
    let alive = true;
    let timer = 0;
    let running = false;
    let again = false;
    const load = () => {
      if (running) { again = true; return; }
      running = true;
      api.gitStatus(path)
        .then((s) => alive && setSt({ path, st: s }))
        .catch(() => alive && setSt({ path, st: null }))
        .finally(() => {
          running = false;
          if (again && alive) { again = false; load(); }
        });
    };
    const soon = () => { clearTimeout(timer); timer = window.setTimeout(load, 400); };
    load();
    const off = onFolderChanged(path, soon);
    window.addEventListener("focus", soon);
    gitSubs.add(soon);
    return () => {
      alive = false;
      clearTimeout(timer);
      off();
      gitSubs.delete(soon);
      window.removeEventListener("focus", soon);
    };
  }, [path, enabled]);
  return st && st.path === path ? st.st : null;
}

// ── items a .gallignore hides from search (dimmed in the list) ──

const hiddenSubs = new Set<() => void>();
/** Reloads every folder's hidden-from-search state (after Hide / Show in search). */
export function bumpSearchHidden() {
  hiddenSubs.forEach((f) => f());
}

export function useSearchHidden(path: string): HiddenState | null {
  const [st, setSt] = useState<{ path: string; st: HiddenState | null } | null>(null);
  useEffect(() => {
    if (!path || archiveOf(path)) {
      setSt(null);
      return;
    }
    let alive = true;
    let timer = 0;
    const load = () =>
      api.searchHidden(path)
        .then((s) => alive && setSt({ path, st: s }))
        .catch(() => alive && setSt({ path, st: null }));
    const soon = () => { clearTimeout(timer); timer = window.setTimeout(load, 300); };
    load();
    const off = onFolderChanged(path, soon);
    hiddenSubs.add(load);
    return () => {
      alive = false;
      clearTimeout(timer);
      off();
      hiddenSubs.delete(load);
    };
  }, [path]);
  return st && st.path === path ? st.st : null;
}
