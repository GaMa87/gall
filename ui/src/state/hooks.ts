// Data hooks: folder listings with a shared cache, auto-refresh from the
// backend watcher, and per-folder preferences.
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "../api/ipc";
import type { FileEntry, GitStatus, HiddenState, ResolvedPrefs, Settings } from "../api/types";
import { archiveOf } from "../core/archive";
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
export function useDirList(path: string | null, settings: Settings | null, dirsOnly = false): DirState {
  const showHidden = settings?.showHidden ?? false;
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

/** Observes an element's size. */
export function useSize<T extends HTMLElement>(): [React.RefObject<T | null>, { w: number; h: number }] {
  const ref = useRef<T | null>(null);
  const observed = useRef<T | null>(null);
  const ro = useRef<ResizeObserver | null>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  // Re-check after every render: the element may appear later or be replaced.
  useLayoutEffect(() => {
    const el = ref.current;
    if (el === observed.current) return;
    ro.current?.disconnect();
    observed.current = el;
    if (!el) return;
    ro.current = new ResizeObserver(([e]) => setSize({ w: e.contentRect.width, h: e.contentRect.height }));
    ro.current.observe(el);
    setSize({ w: el.clientWidth, h: el.clientHeight });
  });
  useEffect(
    () => () => {
      ro.current?.disconnect();
      // Forget the element too, so a re-run (React StrictMode remount) observes it again
      // instead of keeping the stale first-render size forever.
      observed.current = null;
    },
    [],
  );
  return [ref, size];
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
    return () => {
      alive = false;
      clearTimeout(timer);
      off();
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
