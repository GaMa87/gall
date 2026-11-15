// Shortcut labels that match the current key mode: vim keys when they are on, the
// (possibly rebound) chords when they are off. What a label says is what the key does.
import { useMemo } from "react";
import { buildKeymap, keyboardProfile, platformLabel, type Action } from "../core/keymap";
import { keysForAction, shortcutLabel, type KeyLabelCtx } from "../core/vim";
import { useStore } from "./store";

export function useKeyLabels() {
  const settings = useStore((s) => s.settings);
  return useMemo(() => {
    const vim = settings?.vimKeys ?? false;
    const ctx: KeyLabelCtx = {
      vim,
      keymap: buildKeymap(settings?.shortcuts ?? {}, keyboardProfile(settings)),
      leader: vim ? settings?.leaderKey || "Space" : settings?.leaderKeyBasic ?? "Ctrl Ctrl",
    };
    return {
      /** Label for a menu item written with its default chord. */
      chord: (c: string) => {
        const l = shortcutLabel(c, ctx);
        return l && platformLabel(l);
      },
      action: (a: Action) => {
        const l = keysForAction(a, ctx);
        return l && platformLabel(l);
      },
    };
  }, [settings?.keyboardProfile, settings?.vimKeys, settings?.shortcuts, settings?.leaderKey, settings?.leaderKeyBasic]);
}
