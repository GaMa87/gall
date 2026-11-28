// Helpers for actions.d context actions, "Connect to server…" and zoxide suggestions.
import { api } from "../api/ipc";
import type { ActionDef, ActionTarget, FileEntry } from "../api/types";
import { keyActions, type Keyed } from "./actionsKeys";
import { basename, isInside } from "./path";
import { getState, setState, type MenuItem } from "../state/store";

/** Primary URI schemes shown in "Connect to server…" hints. */
export const SERVER_SCHEMES = ["smb", "sftp", "nfs", "davs", "ftp"] as const;
const ACCEPTED_SERVER_SCHEMES = new Set<string>([
  ...SERVER_SCHEMES,
  "ssh",
  "dav",
  "webdav",
  "webdavs",
  "ftps",
  "ftpes",
  "afp",
]);

/** Checks a typed server address; the error text is shown to the user. */
export function validateServerUri(input: string): { ok: true; uri: string } | { ok: false; error: string } {
  const uri = input.trim();
  if (!uri) return { ok: false, error: "Enter a server address, for example smb://server/share" };
  const m = /^([a-z][a-z0-9+.-]*):\/\/(.*)$/i.exec(uri);
  if (!m) return { ok: false, error: `Start the address with a scheme: ${SERVER_SCHEMES.map((s) => `${s}://`).join(", ")}` };
  const scheme = m[1].toLowerCase();
  if (!ACCEPTED_SERVER_SCHEMES.has(scheme)) return { ok: false, error: `“${scheme}://” is not supported. Use ${SERVER_SCHEMES.map((s) => `${s}://`).join(", ")}` };
  if (!m[2].trim() || m[2].startsWith("/")) return { ok: false, error: "The address needs a server name after the scheme" };
  if (/[\0\s]/.test(uri)) return { ok: false, error: "The address must not contain spaces" };
  return { ok: true, uri: `${scheme}://${m[2]}` };
}

/** Menu / palette label of an action for `count` selected items (`{count}` is substituted). */
export function actionLabel(def: ActionDef, count: number): string {
  const text = count > 1 && def.pluralLabel ? def.pluralLabel : def.label;
  return text.replaceAll("{count}", String(count));
}

export function toActionTarget(e: Pick<FileEntry, "path" | "isDir" | "ext">): ActionTarget {
  return { path: e.path, isDir: e.isDir, ext: e.ext };
}

/** Actions that match the entries (or the folder itself when none are given). Failures → none. */
export async function actionsForEntries(entries: FileEntry[], dir: string): Promise<ActionDef[]> {
  const targets = entries.length
    ? entries.map(toActionTarget)
    : [{ path: dir, isDir: true, ext: "" } satisfies ActionTarget];
  try {
    return (await api.actionsForSelection(targets)) ?? [];
  } catch {
    return [];
  }
}

export function actionPaths(entries: FileEntry[], dir: string): string[] {
  return entries.length ? entries.map((e) => e.path) : [dir];
}

/** The actions that fit `entries` (or the folder), each with its key in the leader group `x`. */
export async function keyedActionsFor(entries: FileEntry[], dir: string): Promise<{ items: Keyed<ActionDef>[]; count: number }> {
  const defs = await actionsForEntries(entries, dir);
  return { items: keyActions(defs), count: actionPaths(entries, dir).length };
}

/** Runs the action on `key` for `entries` (or the folder). False when no action has that key. */
export async function runKeyedAction(key: string, entries: FileEntry[], dir: string, onError: (e: unknown) => void): Promise<boolean> {
  const { items } = await keyedActionsFor(entries, dir);
  const hit = items.find((i) => i.key === key);
  if (!hit) return false;
  await api.actionRun(hit.def.id, actionPaths(entries, dir), dir).catch(onError);
  return true;
}

/** Menu items running `defs` on the given entries / folder. */
export function actionMenuItems(defs: ActionDef[], entries: FileEntry[], dir: string, onError: (e: unknown) => void): MenuItem[] {
  const paths = actionPaths(entries, dir);
  return defs.map((d) => ({
    label: actionLabel(d, paths.length),
    run: () => { api.actionRun(d.id, paths, dir).catch(onError); },
  }));
}

/**
 * Appends the matching actions to a menu that is already on screen. The menu is built
 * synchronously; the actions arrive a moment later and are added only if that very menu is
 * still open (a newer menu or a closed one is left alone).
 */
export async function appendActionsToMenu(
  shown: MenuItem[],
  entries: FileEntry[],
  dir: string,
  onError: (e: unknown) => void,
  updateMenu?: (updater: (items: MenuItem[]) => MenuItem[]) => void,
) {
  const defs = await actionsForEntries(entries, dir);
  if (!defs.length) return;
  const extra: MenuItem[] = [{ separator: true }, ...actionMenuItems(defs, entries, dir, onError)];
  if (updateMenu) {
    updateMenu((cur) => [...cur, ...extra]);
    return;
  }
  const cur = getState().menu;
  if (!cur || cur.items !== shown) return;
  setState({ menu: { ...cur, items: [...shown, ...extra] } });
}

/** Zoxide folders matching `matches(path)` that are not already among `have`, within `scope` when given. */
export function mergeZoxide(zoxide: string[], have: Iterable<string>, matches: (path: string) => boolean, scope: string | null, limit = 10): string[] {
  const seen = new Set(have);
  const out: string[] = [];
  for (const p of zoxide) {
    if (out.length >= limit) break;
    if (seen.has(p) || (scope && !isInside(p, scope)) || !matches(p)) continue;
    seen.add(p);
    out.push(p);
  }
  return out;
}

/** Display name of a zoxide folder. */
export const zoxideName = (p: string) => basename(p) || p;
