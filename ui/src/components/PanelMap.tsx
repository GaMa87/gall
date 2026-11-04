// Panel controls in the title bar, right above the divider between the browsers:
// ‹ / › hide one browser so the other takes the whole window (and bring both back),
// ⇄ swaps their folders while each keeps its own panes. Right-click for a menu that
// also toggles each browser's preview & details pane.
import { useState } from "react";
import type { Action } from "../core/keymap";
import * as A from "../state/actions";
import { useKeyLabels } from "../state/keyLabels";
import { useStore } from "../state/store";
import { Icon } from "./Icon";

const SIDE = ["Left", "Right"] as const;

export function PanelMap() {
  const session = useStore((s) => s.session);
  const settings = useStore((s) => s.settings);
  const keys = useKeyLabels();
  const [swapping, setSwapping] = useState(0);
  if (!session || !settings) return null;

  const layout = session.layout;
  const vertical = layout === "columns";
  const classic = layout === "classic";
  const side = (b: number) => (vertical ? (b ? "Bottom" : "Top") : SIDE[b]);
  const k = (a: Action) => {
    const x = keys.action(a);
    return x ? ` (${x})` : "";
  };
  const swap = () => {
    A.swapBrowsers();
    setSwapping((n) => n + 1); // new key restarts the animation on repeated swaps
  };
  const restore = (b: number) => A.sessionCmd({ op: "collapse", b, collapsed: false });
  const collapse = (b: number) => A.sessionCmd({ op: "collapse", b, collapsed: true });
  const anyCollapsed = session.browsers.some((x) => x.collapsed);

  const menu = (e: React.MouseEvent) => {
    e.preventDefault();
    A.showMenu(e.clientX, e.clientY, [
      ...(classic ? [] : [{ label: "Swap folders", icon: "swap" as const, shortcut: "Ctrl+U", run: swap }, { separator: true as const }]),
      ...(classic ? [session.activeBrowser] : [0, 1]).map((b) => ({
        label: classic ? "Preview & details pane" : `Preview & details pane: ${side(b).toLowerCase()}`,
        checked: A.detailsShown({ session, settings }, b),
        run: () => A.toggleDetails(b),
      })),
      ...(classic
        ? []
        : [
            { separator: true as const },
            anyCollapsed
              ? { label: "Show both browsers", run: () => [0, 1].forEach(restore) }
              : { label: `Hide ${side(0).toLowerCase()} browser`, run: () => collapse(0) },
            ...(anyCollapsed ? [] : [{ label: `Hide ${side(1).toLowerCase()} browser`, run: () => collapse(1) }]),
          ]),
    ]);
  };

  if (classic) return null; // one browser: nothing to hide or swap

  // ‹ moves the divider to the left end: the left browser hides and the right one takes
  // the whole width; pressed again from the other end it brings both back. › mirrors it.
  const [c0, c1] = [session.browsers[0].collapsed, session.browsers[1].collapsed];
  const arrow = (b: 0 | 1) => {
    const other = (1 - b) as 0 | 1;
    const otherHidden = b === 0 ? c1 : c0;
    const selfHidden = b === 0 ? c0 : c1;
    const glyph = vertical ? (b === 0 ? "˄" : "˅") : b === 0 ? "‹" : "›";
    return (
      <button
        className="pm-arrow"
        disabled={selfHidden}
        title={otherHidden ? "Show both browsers" : `Hide the ${side(b).toLowerCase()} browser; the ${side(other).toLowerCase()} one takes the whole window`}
        onClick={() => (otherHidden ? restore(other) : collapse(b))}
      >
        {glyph}
      </button>
    );
  };
  return (
    <div className={`panel-map ${vertical ? "vertical" : ""}`} onContextMenu={menu} role="group" aria-label="Panels">
      {arrow(0)}
      <button key={swapping} className={`pm-swap ${swapping ? "spin" : ""}`} title={`Swap folders of the two browsers; each keeps its own panes${k("swapBrowsers")}`} onClick={swap}>
        <Icon name="swap" size={15} />
      </button>
      {arrow(1)}
    </div>
  );
}
