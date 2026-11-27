import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { CommitQuery } from "../core/gitSearch";
import { createProgressId } from "../core/gitProgress";

export type CommitSearch = { lines: string; running: boolean; truncated: boolean; error: string; progressId: string | null; cancel: () => void };
const noop = () => {};
const idle: CommitSearch = { lines: "", running: false, truncated: false, error: "", progressId: null, cancel: noop };
/** How long typing has to pause before Git is asked. */
const DEBOUNCE_MS = 220;
let tickets = Date.now();

/** Searches the whole history for `query` in the background; typing on or leaving stops the search that runs. */
export function useCommitSearch(path: string, query: CommitQuery | null, reflog: boolean, reference: string, revision: unknown): CommitSearch {
  const [state, setState] = useState<CommitSearch>(idle);
  const key = query ? JSON.stringify(query) : "";
  const latest = useRef(query); latest.current = query;
  useEffect(() => {
    if (!key) { setState(idle); return; }
    const ticket = ++tickets;
    const progressId = createProgressId("search");
    let active = true, started = false, unlisten = () => {};
    const cancel = () => {
      active = false;
      window.clearTimeout(timer);
      unlisten();
      if (started) void invoke("git_search_cancel", { ticket }).catch(() => {});
      setState((s) => ({ ...s, running: false, progressId: null }));
    };
    setState({ ...idle, running: true, progressId, cancel });
    const timer = window.setTimeout(() => {
      started = true;
      const listening = listen<{ ticket: number; lines: string }>("git-search-hits", (event) => {
        if (active && event.payload.ticket === ticket) setState((s) => ({ ...s, lines: s.lines + (s.lines && !s.lines.endsWith("\n") ? "\n" : "") + event.payload.lines }));
      });
      void listening.then((stop) => {
        unlisten = stop;
        if (!active) { stop(); return; }
        invoke<{ truncated: boolean }>("git_search", { path, query: latest.current, reflog, reference: reference || null, ticket, progressId })
          .then((end) => { if (active) setState((s) => ({ ...s, running: false, progressId: null, truncated: end.truncated })); })
          .catch((error) => { if (active) setState((s) => ({ ...s, running: false, progressId: null, error: String(error) })); });
      });
    }, DEBOUNCE_MS);
    return () => {
      active = false; window.clearTimeout(timer); unlisten();
      if (started) void invoke("git_search_cancel", { ticket }).catch(() => {});
    };
    // A refresh (`revision`) searches again: commits may have been added or rewritten.
  }, [path, key, reflog, reference, revision]);
  return state;
}
