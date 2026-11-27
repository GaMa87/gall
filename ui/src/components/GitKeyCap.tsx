import type { GitScope } from "../core/gitCommands";
import { useGitKeys } from "../state/gitKeys";
import { tt } from "../i18n";

/** The key of a Git command on its button, as it is bound now; nothing when no key runs it. */
export function GitKeyCap({ scope, k }: { scope: GitScope; k: string }) {
  const cap = useGitKeys().cap(scope, k);
  return cap ? <> <kbd>{cap === "Space" ? tt("Space") : cap}</kbd></> : null;
}
