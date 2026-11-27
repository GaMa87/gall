import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { age, ageWords } from "../core/age";
import { BlameCache, BlameLines, uncommitted, type BlameLine, type BlamePart } from "../core/gitBlame";
import { createProgressId } from "../core/gitProgress";
import { formatLocale, tt } from "../i18n";

/** What to blame: a file of a repository in a commit, or a file on disk (`absolute`, without a commit the one on disk). */
export type BlameRequest = { path: string; commit: string | null; file: string; absolute?: boolean };
export type Blame = { lines: (BlameLine | undefined)[]; done: boolean; limited: boolean; ignored: boolean; error: string; progressId: string | null; cancel: () => void };

const cache = new BlameCache<Blame>();
let tickets = Date.now();

/** Blame of a file, filled in as Git finds it; null while nothing is asked. Leaving stops Git. */
export function useBlame(request: BlameRequest | null): Blame | null {
  const key = request ? JSON.stringify([request.path, request.commit, request.file, !!request.absolute]) : "";
  const [state, setState] = useState<{ key: string; blame: Blame } | null>(null);
  const latest = useRef(request); latest.current = request;
  useEffect(() => {
    const asked = latest.current;
    if (!asked) return;
    const known = asked.commit ? cache.get(key) : undefined;
    if (known) { setState({ key, blame: known }); return; }
    let alive = true, frame = 0;
    const ticket = ++tickets, lines = new BlameLines(), progressId = createProgressId("blame");
    const cancel = () => {
      alive = false;
      cancelAnimationFrame(frame);
      void stop.then((off) => off());
      invoke("git_blame_cancel", { ticket }).catch(() => {});
      setState((prev) => prev && prev.key === key ? { key, blame: { ...prev.blame, done: true, progressId: null } } : prev);
    };
    const show = (rest: Partial<Blame> = {}) => {
      const blame: Blame = { lines: lines.lines.slice(), done: false, limited: false, ignored: false, error: "", progressId, cancel, ...rest };
      setState({ key, blame });
      return blame;
    };
    show();
    const stop = listen<{ ticket: number; part: BlamePart }>("git-blame-part", (event) => {
      if (!alive || event.payload.ticket !== ticket) return;
      lines.add(event.payload.part);
      // Parts come faster than frames.
      frame ||= requestAnimationFrame(() => { frame = 0; if (alive) show(); });
    });
    void stop.then(() => alive ? invoke<{ limited: boolean; ignored: boolean }>("git_blame_lines", { ...asked, absolute: !!asked.absolute, ticket, progressId }) : null).then((end) => {
      if (!alive || !end) return;
      // The last parts may still be on their way: they are events, the end is an answer.
      setTimeout(() => {
        if (!alive) return;
        cancelAnimationFrame(frame); frame = 0;
        const blame = show({ ...end, done: true, progressId: null });
        if (asked.commit) cache.set(key, blame);
      }, 60);
    }).catch((e) => { if (alive) show({ done: true, progressId: null, error: String(e) }); });
    return () => {
      alive = false;
      cancelAnimationFrame(frame);
      void stop.then((off) => off());
      invoke("git_blame_cancel", { ticket }).catch(() => {});
    };
  }, [key]);
  return state && state.key === key && request ? state.blame : null;
}

export const blameShort = (line: BlameLine, now: number) => uncommitted(line.commit) ? tt("Not committed yet") : `${line.commit.author} · ${age(line.commit.time * 1000, now).label}`;

export function blameTitle(line: BlameLine, now: number): string {
  if (uncommitted(line.commit)) return tt("Not committed yet");
  const time = line.commit.time * 1000;
  return [line.commit.subject, `${line.commit.id}`, `${line.commit.author} · ${new Date(time).toLocaleString(formatLocale())} (${tt("{value} ago", { value: ageWords(time, now) })})`, tt("Line {line} of {file}", { line: line.original, file: line.file })].join("\n");
}
