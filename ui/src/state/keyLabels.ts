// Shortcut labels that match the current key mode: vim keys when they are on, the
// (possibly rebound) chords when they are off. What a label says is what the key does.
import { useMemo } from "react";
import { buildKeymap, type Action } from "../core/keymap";
import { keysForAction, shortcutLabel, type KeyLabelCtx } from "../core/vim";
import { useStore } from "./store";

export function useKeyLabels() {
  const settings = useStore((s) => s.settings);
  return useMemo(() => {
    const vim = settings?.vimKeys ?? false;
    const ctx: KeyLabelCtx = {
      vim,
      keymap: buildKeymap(settings?.shortcuts ?? {}),
      leader: vim ? settings?.leaderKey || "Space" : settings?.leaderKeyBasic ?? "Ctrl Ctrl",
    };
    return {
      /** Label for a menu item written with its default chord. */
      chord: (c: string) => shortcutLabel(c, ctx),
      action: (a: Action) => keysForAction(a, ctx),
    };
  }, [settings?.vimKeys, settings?.shortcuts, settings?.leaderKey, settings?.leaderKeyBasic]);
}
