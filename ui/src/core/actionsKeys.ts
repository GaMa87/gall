// The keys of the leader group "custom actions" (actions.d): 1–9, 0, then a–z, in the order the
// actions are listed. Kept apart from `actionsMenu.ts` so the key tree (`vim.ts`) stays free of the
// backend and the store.

/** Keys that can pick an action, in listing order. */
export const ACTION_KEYS = "1234567890abcdefghijklmnopqrstuvwxyz";

export interface Keyed<T> {
  key: string;
  def: T;
}

/** Gives each action its key; actions beyond the last key are left out of the group (the palette lists them all). */
export function keyActions<T>(defs: readonly T[]): Keyed<T>[] {
  return defs.slice(0, ACTION_KEYS.length).map((def, i) => ({ key: ACTION_KEYS[i], def }));
}
