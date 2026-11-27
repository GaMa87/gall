// Panel controls in the title bar, right above the divider between the browsers:
// ‹ / › hide one browser so the other takes the whole window (and bring both back),
// ⇄ swaps their folders while each keeps its own panes, and the live Pair Bridge
// links/cycles Linked (⇄), Tandem (🔗), and Flow (→ / ←) modes (WN-180, Variant A).
import { useState } from "react";
import { t, tt } from "../i18n";
import type { Action } from "../core/keymap";
import { activePair, pairColor, pairGlyph, pairModeLabel } from "../core/pairs";
import * as A from "../state/actions";
import { draggedTab } from "../state/dnd";
import { useKeyLabels } from "../state/keyLabels";
import { arrangement, setState, useStore } from "../state/store";
import { Icon } from "./Icon";

const SIDE = ["Left", "Right"] as const;

export function PanelMap() {
  const session = useStore((s) => s.session);
  const settings = useStore((s) => s.settings);
  const keys = useKeyLabels();
  const [swapping, setSwapping] = useState(0);
  const [bridgeDrop, setBridgeDrop] = useState(false);
  const vertical = useStore((s) => arrangement(s).vertical);
  // One browser shown because the window is small (adaptive layout): the arrows switch browsers.
  const autoSingle = useStore((s) => arrangement(s).autoSingle);
  if (!session || !settings) return null;

  const layout = session.layout;
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
  const pairsEnabled = settings.liveTabPairs ?? true;
  const pair = pairsEnabled ? activePair(session) : null;
  const pColor = pair ? pairColor(pair) : undefined;

  const bridgeMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (!pair) {
      A.showMenuFor(e, [
        { label: `${t("pair.linked")} (⇄)`, run: () => A.pairActiveTabs("linked") },
        { label: `${t("pair.tandem")} (🔗)`, run: () => A.pairActiveTabs("tandem") },
        { separator: true },
        { label: `${t("pair.flowCopy")} (→)`, run: () => A.pairActiveTabs("flow", "leftToRight", "copy") },
        { label: `${t("pair.flowMove")} (→)`, run: () => A.pairActiveTabs("flow", "leftToRight", "move") },
        { label: `${t("pair.flowCopy")} (←)`, run: () => A.pairActiveTabs("flow", "rightToLeft", "copy") },
        { label: `${t("pair.flowMove")} (←)`, run: () => A.pairActiveTabs("flow", "rightToLeft", "move") },
      ]);
      return;
    }
    A.showMenuFor(e, [
      { label: `${t("pair.linked")} (⇄)`, checked: pair.mode === "linked", run: () => A.setPairMode(pair.id, "linked") },
      { label: `${t("pair.tandem")} (🔗)`, checked: pair.mode === "tandem", run: () => A.setPairMode(pair.id, "tandem") },
      { separator: true },
      {
        label: `${t("pair.flowCopy")} (→)`,
        checked: pair.mode === "flow" && pair.flowDir === "leftToRight" && pair.flowOp === "copy",
        run: () => A.setPairMode(pair.id, "flow", "leftToRight", "copy"),
      },
      {
        label: `${t("pair.flowMove")} (→)`,
        checked: pair.mode === "flow" && pair.flowDir === "leftToRight" && pair.flowOp === "move",
        run: () => A.setPairMode(pair.id, "flow", "leftToRight", "move"),
      },
      {
        label: `${t("pair.flowCopy")} (←)`,
        checked: pair.mode === "flow" && pair.flowDir === "rightToLeft" && pair.flowOp === "copy",
        run: () => A.setPairMode(pair.id, "flow", "rightToLeft", "copy"),
      },
      {
        label: `${t("pair.flowMove")} (←)`,
        checked: pair.mode === "flow" && pair.flowDir === "rightToLeft" && pair.flowOp === "move",
        run: () => A.setPairMode(pair.id, "flow", "rightToLeft", "move"),
      },
      { separator: true },
      ...(pair.mode === "tandem" ? [{ label: t("pair.reanchor"), run: () => A.reanchorPair(pair.id) }] : []),
      ...(pair.mode === "flow" ? [{ label: t("pair.sendFlow"), shortcut: keys.action("pairFlowSend") || undefined, run: () => { void A.sendAcrossFlow(pair.id); } }] : []),
      { label: t("pair.unpair"), shortcut: keys.action("pairToggle") || undefined, run: () => A.unpairTabs(pair.id) },
    ]);
  };

  const menu = (e: React.MouseEvent) => {
    e.preventDefault();
    A.showMenuFor(e, [
      ...(classic ? [] : [{ label: "Swap folders", icon: "swap" as const, shortcut: "Ctrl+U", run: swap }, { separator: true as const }]),
      ...(classic ? [session.activeBrowser] : [0, 1]).map((b) => ({
        label: classic ? "Preview & details pane" : `Preview & details pane: ${side(b).toLowerCase()}`,
        checked: A.detailsShown({ session, settings }, b),
        run: () => A.toggleDetails(b),
      })),
      { separator: true as const },
      ...(classic ? [session.activeBrowser] : [0, 1]).map((b) => ({
        label: classic ? tt("Show hidden files") : tt(`Show hidden files: ${side(b).toLowerCase()}`),
        checked: A.hiddenShown({ session, settings }, b),
        run: () => A.toggleHidden(b),
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
    if (autoSingle) {
      const shown = session.activeBrowser === b;
      return (
        <button className={`pm-arrow${shown ? " on" : ""}`} aria-pressed={shown} disabled={shown}
          title={shown ? `The ${side(b).toLowerCase()} browser is shown (the window is small: one browser at a time)` : `Show the ${side(b).toLowerCase()} browser`}
          onClick={() => { A.focusBrowser(b); }}>
          {glyph}
        </button>
      );
    }
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
    <div className={`panel-map ${vertical ? "vertical" : ""}`} onContextMenu={menu} role="group" aria-label={tt("Panels")}>
      {arrow(0)}
      <button key={swapping} className={`pm-swap ${swapping ? "spin" : ""}`} title={`Swap folders of the two browsers; each keeps its own panes${k("swapBrowsers")}`} onClick={swap}>
        <Icon name="swap" size={15} />
      </button>
      {pairsEnabled && !autoSingle && !anyCollapsed && (
        <div
          className={`pm-bridge${pair ? ` paired mode-${pair.mode}` : ""}${bridgeDrop ? " drop" : ""}`}
          style={pColor ? ({ "--pair-color": pColor } as React.CSSProperties) : undefined}
          onContextMenu={bridgeMenu}
          onMouseDown={(e) => {
            if (pair && e.button === 1) {
              e.preventDefault();
              e.stopPropagation();
            }
          }}
          onMouseUp={(e) => {
            if (pair && e.button === 1) {
              e.preventDefault();
              e.stopPropagation();
              A.unpairTabs(pair.id);
            }
          }}
          onAuxClick={(e) => {
            if (pair && e.button === 1) {
              e.preventDefault();
              e.stopPropagation();
            }
          }}
          onMouseEnter={() => { if (pair) setState({ hoveredPairId: pair.id }); }}
          onMouseLeave={() => { if (pair) setState({ hoveredPairId: null }); }}
          onDragOver={(e) => {
            if (draggedTab()) {
              e.preventDefault();
              setBridgeDrop(true);
            }
          }}
          onDragLeave={() => setBridgeDrop(false)}
          onDrop={(e) => {
            setBridgeDrop(false);
            const src = draggedTab();
            if (!src) return;
            e.preventDefault();
            const leftIdx = src.b === 0 ? src.idx : session.browsers[0].active;
            const rightIdx = src.b === 1 ? src.idx : session.browsers[1].active;
            A.pairTabs(leftIdx, rightIdx, pair?.mode ?? "linked");
          }}
        >
          <button
            type="button"
            className="pm-bridge-btn"
            aria-pressed={!!pair}
            title={
              pair
                ? `${pairModeLabel(pair.mode, pair.flowDir, pair.flowOp)} — ${
                    pair.mode === "tandem"
                      ? t("pair.descTandem")
                      : pair.mode === "flow"
                        ? t("pair.descFlow")
                        : t("pair.descLinked")
                  }\n${t("pair.bridgeHint")}${k("pairCycleMode")}`
                : `${t("pair.pairActive")}${k("pairToggle")}\n${t("pair.pairActiveHint")}`
            }
            onClick={() => (pair ? A.cycleActivePairMode(pair.id) : A.toggleActivePair())}
          >
            {pair ? <span className="pm-bridge-glyph">{pairGlyph(pair)}</span> : <Icon name="link" size={13} />}
          </button>
          {pair?.mode === "flow" && (
            <button
              type="button"
              className="pm-flow-send"
              title={`${t("pair.sendFlow")}${k("pairFlowSend")}\n${t("pair.sendFlowHint")}`}
              onClick={() => { void A.sendAcrossFlow(pair.id); }}
            >
              {t("pair.sendShort")}
            </button>
          )}
          {pair && (
            <button
              type="button"
              className="pm-bridge-unlink"
              title={`${t("pair.unpair")}${k("pairToggle")}\n${t("pair.unpairHint")}`}
              aria-label={t("pair.unpair")}
              onClick={(e) => {
                e.stopPropagation();
                A.unpairTabs(pair.id);
              }}
            >
              ×
            </button>
          )}
        </div>
      )}
      {arrow(1)}
    </div>
  );
}
