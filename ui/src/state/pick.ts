// The file picker window: Enter / double click on a file answers instead of opening it.
import type { FileEntry } from "../api/types";
import { api } from "../api/ipc";
import { accept } from "../components/PickerBar";
import * as A from "./actions";
import { getState } from "./store";

/** What is at `path`: null when nothing, else whether it is a folder. */
export const exists = async (path: string): Promise<{ isDir: boolean } | null> => {
  const r = await api.pathResolve(path).catch(() => null);
  return r?.exists ? { isDir: r.isDir } : null;
};

/** `openEntries` in a picker window. Returns true when it handled the entries. */
export function pickOpenEntries(b: number, entries: FileEntry[]): boolean {
  const pick = getState().pick;
  if (!pick) return false;
  const dirs = entries.filter((e) => e.isDir);
  // A folder opens in the pane, as usual: that is how the user finds the file.
  if (dirs.length === 1 && entries.length === 1) {
    A.navigate(b, dirs[0].path);
    return true;
  }
  if (pick.mode === "save") {
    // Enter on a file in a save dialog takes its name, to replace it.
    const f = entries.find((e) => !e.isDir);
    if (f) void accept(f.name);
    return true;
  }
  void accept("");
  return true;
}
