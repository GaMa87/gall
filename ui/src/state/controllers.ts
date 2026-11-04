// Registry of live browser controllers so global commands (keyboard, sidebar,
// copy-to-other-browser) can talk to the folder pane of browser 0 or 1.
import type { FileEntry } from "../api/types";
import type { Action } from "../core/keymap";

export interface BrowserController {
  path(): string;
  entries(): FileEntry[];
  selection(): FileEntry[];
  focused(): FileEntry | null;
  reload(): void;
  select(paths: string[], focusPath?: string): void;
  focus(): void;
  /** Handles list-local actions; returns true if consumed. */
  handle(a: Action): boolean;
  /** Vim visual mode is on (single y / d / x then act at once, as in Vim). */
  visual?(): boolean;
}

const controllers: (BrowserController | null)[] = [null, null];

export function registerController(b: number, c: BrowserController | null) {
  controllers[b] = c;
}

export function controller(b: number): BrowserController | null {
  return controllers[b] ?? null;
}

// fs-changed fan-out: components subscribe to the folders they display.
type FsCb = (storm: boolean) => void;
const fsSubs = new Map<string, Set<FsCb>>();

export function onFolderChanged(path: string, cb: FsCb): () => void {
  let s = fsSubs.get(path);
  if (!s) fsSubs.set(path, (s = new Set()));
  s.add(cb);
  return () => {
    s!.delete(cb);
    if (!s!.size) fsSubs.delete(path);
  };
}

export function emitFolderChanged(path: string, storm: boolean) {
  fsSubs.get(path)?.forEach((cb) => cb(storm));
}

/** Forces every displayed folder to reload (F5 / "Refresh everything"). */
export function refreshAll() {
  for (const [, subs] of fsSubs) subs.forEach((cb) => cb(false));
}
