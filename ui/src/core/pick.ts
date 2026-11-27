// Rules of the file picker window (`gall --pick`): what the user may choose and how the answer looks.
// Pure functions so the rules are unit tested; the window itself is components/PickerBar.tsx.
import type { PickArgs, PickFilter } from "../api/types";
import { join } from "./path";

export interface Chosen {
  path: string;
  name: string;
  isDir: boolean;
}

export type PickCheck = { ok: true; paths: string[] } | { ok: false; error: string };

/** `*.png` / `report-??.txt` against a file name, ignoring case. */
export function globMatch(name: string, pattern: string): boolean {
  const re = pattern
    .replace(/[.+^${}()|[\]\\]/g, "\\$&")
    .replace(/\*/g, ".*")
    .replace(/\?/g, ".");
  return new RegExp(`^${re}$`, "i").test(name);
}

/** A filter without patterns lets every file through. */
export function matchesFilter(name: string, filter: PickFilter | undefined): boolean {
  if (!filter || !filter.patterns.length) return true;
  return filter.patterns.some((p) => globMatch(name, p));
}

/** `name` is a plain file name (no folder part) that can be created in a folder. */
export function validFileName(name: string): boolean {
  return !!name && name !== "." && name !== ".." && !/[\\/\0]/.test(name);
}

/** What happens when the user presses the accept button (or Enter on a file). */
export function checkPick(
  pick: Pick<PickArgs, "mode" | "multiple" | "filters">,
  ctx: { dir: string; selection: Chosen[]; typedName: string; filter: number },
): PickCheck {
  const { mode, multiple } = pick;
  const filter = pick.filters[ctx.filter];
  if (mode === "save") {
    const name = ctx.typedName.trim();
    if (!name) return { ok: false, error: "Type a file name" };
    if (!validFileName(name)) return { ok: false, error: "A file name cannot contain / or \\" };
    return { ok: true, paths: [join(ctx.dir, name)] };
  }
  if (mode === "dir") {
    const dirs = ctx.selection.filter((e) => e.isDir);
    if (ctx.selection.length && dirs.length !== ctx.selection.length) return { ok: false, error: "Select a folder" };
    if (!dirs.length) return { ok: true, paths: [ctx.dir] };
    if (dirs.length > 1 && !multiple) return { ok: false, error: "Select one folder" };
    return { ok: true, paths: dirs.map((d) => d.path) };
  }
  const files = ctx.selection.filter((e) => !e.isDir);
  if (!ctx.selection.length) return { ok: false, error: "Select a file" };
  if (files.length !== ctx.selection.length) return { ok: false, error: "Select files, not folders" };
  if (files.length > 1 && !multiple) return { ok: false, error: "Select one file" };
  const bad = files.find((f) => !matchesFilter(f.name, filter));
  if (bad) return { ok: false, error: `“${bad.name}” does not match “${filter?.name}”` };
  return { ok: true, paths: files.map((f) => f.path) };
}

/** The accept button's text: the caller's label, else by mode. */
export function acceptLabel(pick: Pick<PickArgs, "mode" | "accept">): string {
  return pick.accept?.replace(/_/g, "") || (pick.mode === "save" ? "Save" : pick.mode === "dir" ? "Select" : "Open");
}
