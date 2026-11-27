import { Fragment } from "react";
import { platformLabel } from "../core/keymap";

export type KeySequence = readonly string[];

const KEY_NAMES: Record<string, string> = {
  ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→",
  PageUp: "PgUp", PageDown: "PgDn", Escape: "Esc",
};

/** The text of one key cap: arrows as arrows, Ctrl and Alt as Cmd and Option on macOS. */
export function keyCap(key: string, mac?: boolean): string {
  return platformLabel(KEY_NAMES[key] ?? key, mac);
}

function chordKeys(chord: string): string[] {
  const keys: string[] = [];
  let rest = chord;
  let match: RegExpExecArray | null;
  while ((match = /^(Ctrl|Shift|Alt|Cmd|Meta)\+(.+)$/i.exec(rest))) {
    keys.push(match[1]);
    rest = match[2];
  }
  return [...keys, rest];
}

export function vimSteps(tokens: readonly string[], leader = "Ctrl"): string[] {
  return tokens.flatMap((token) => token === "CtrlTap" ? leader.split(" ")
    : /^[A-Z]$/.test(token) ? [`Shift+${token.toLowerCase()}`] : [token]);
}

/** Alternatives contain sequences; each sequence step is one simultaneous chord. */
export function ShortcutKeys({ alternatives }: { alternatives: readonly KeySequence[] }) {
  const description = alternatives.map((steps) => steps.map((step) => {
    const keys = chordKeys(step);
    const names = keys.map((k) => keyCap(k));
    return keys.length > 1 ? `${names.join(" and ")} together` : names[0];
  }).join(", then ")).join(", or ");
  return (
    <span className="key-bindings" role="img" aria-label={description} title={description}>
      {alternatives.map((steps, option) => (
        <span className="key-option" key={option} aria-hidden="true">
          {option > 0 && <span className="key-or" aria-hidden="true">or</span>}
          <span className="key-route" aria-hidden="true">
            {steps.map((step, index) => (
              <Fragment key={index}>
                {index > 0 && <span className="key-then">→</span>}
                <span className="key-chord">
                  {chordKeys(step).map((key, part) => (
                    <Fragment key={part}>
                      {part > 0 && <span className="key-plus">+</span>}
                      <kbd>{keyCap(key)}</kbd>
                    </Fragment>
                  ))}
                </span>
              </Fragment>
            ))}
          </span>
        </span>
      ))}
    </span>
  );
}
