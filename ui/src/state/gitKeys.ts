// The Git keys as they are bound now, for panels: which command a key means, and what to print on a button.
import { gitHints, gitKeyCap, gitKeyName, gitPanelKey, setCustomGitCommands, type GitContext, type GitScope } from "../core/gitCommands";
import { platformLabel } from "../core/keymap";
import { useStore } from "./store";

const NONE: Record<string, string> = {};
type PanelEvent = Parameters<typeof gitPanelKey>[1];

export function useGitKeys() {
  const shortcuts = useStore((s) => s.settings?.shortcuts) ?? NONE;
  const vim = useStore((s) => s.settings?.vimKeys ?? false);
  // The commands of the user join the registry before anything asks it for keys, hints or menu entries.
  setCustomGitCommands(useStore((s) => s.settings?.gitCommands));
  return {
    vim, shortcuts,
    press: (scopes: GitScope[], event: PanelEvent) => gitPanelKey(scopes, event, shortcuts, vim),
    /** Key cap of the command with this default key; empty when no key runs it. */
    cap: (scope: GitScope, defaultKey: string) => platformLabel(gitKeyCap(scope, defaultKey, shortcuts, vim)),
    hints: (context: GitContext) => gitHints(context, shortcuts).map((hint) => ({ ...hint, name: platformLabel(gitKeyName(hint.key)) })),
  };
}
