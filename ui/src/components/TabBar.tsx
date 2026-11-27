// Browser tabs (T-01…T-10) and Live Tab Pair badges (WN-180, Variant A).
import { t as tr } from "../i18n"; // aliased: `t` is a tab in this file
import { useEffect, useRef, useState } from "react";
import type { BrowserState, TabPair } from "../api/types";
import { findPairForTab, pairColor, pairGlyph, pairModeLabel, partnerTabOf } from "../core/pairs";
import { basename, tildify } from "../core/path";
import * as A from "../state/actions";
import { draggedTab, dropInto, isFileDrag, onDragOverTarget, startTabDrag } from "../state/dnd";
import { useStore } from "../state/store";
import { useKeyLabels } from "../state/keyLabels";
import { Icon } from "./Icon";
import { platformLabel } from "../core/keymap";
import { tt } from "../i18n";


export function TabBar({ b, browser, right }: { b: number; browser: BrowserState; right?: React.ReactNode }) {
  const settings = useStore((s) => s.settings)!;
  const session = useStore((s) => s.session);
  const hoveredPairId = useStore((s) => s.hoveredPairId ?? null);
  const ghost = useStore((s) => s.gravityGhost?.[b] ?? null);
  const home = useStore((s) => s.info?.home ?? "");
  const shownBrowsers = useStore((s) => s.shownBrowsers ?? [0, 1]);
  const keys = useKeyLabels();
  const [armed, setArmed] = useState<number | null>(null);
  const [drop, setDrop] = useState<string | null>(null);
  const [pairDrop, setPairDrop] = useState<string | null>(null);
  const armTimer = useRef<number | null>(null);
  const middlePressed = useRef<number | null>(null);
  useEffect(() => () => { if (armTimer.current) clearTimeout(armTimer.current); }, []);

  const otherB = 1 - b;
  const singlePane = shownBrowsers.length === 1 || session?.layout === "classic";
  const pairsEnabled = settings.liveTabPairs ?? true;
  const gravityEnabled = settings.pairGravity ?? true;

  const pairMenuItems = (idx: number, pair: TabPair | null) => {
    const otherActiveIdx = session?.browsers[otherB]?.active ?? 0;
    const leftIdx = b === 0 ? idx : otherActiveIdx;
    const rightIdx = b === 1 ? idx : otherActiveIdx;
    if (!pair) {
      return [
        {
          label: tr("pair.pairWithOther"),
          icon: "link" as const,
          submenu: [
            { label: `${tr("pair.linked")} (⇄)`, run: () => A.pairTabs(leftIdx, rightIdx, "linked") },
            { label: `${tr("pair.tandem")} (🔗)`, run: () => A.pairTabs(leftIdx, rightIdx, "tandem") },
            { separator: true as const },
            { label: `${tr("pair.flowCopy")} (→)`, run: () => A.pairTabs(leftIdx, rightIdx, "flow", "leftToRight", "copy") },
            { label: `${tr("pair.flowMove")} (→)`, run: () => A.pairTabs(leftIdx, rightIdx, "flow", "leftToRight", "move") },
            { label: `${tr("pair.flowCopy")} (←)`, run: () => A.pairTabs(leftIdx, rightIdx, "flow", "rightToLeft", "copy") },
            { label: `${tr("pair.flowMove")} (←)`, run: () => A.pairTabs(leftIdx, rightIdx, "flow", "rightToLeft", "move") },
          ],
        },
      ];
    }
    return [
      {
        label: tr("pair.bridgeMode"),
        icon: "link" as const,
        submenu: [
          { label: `${tr("pair.linked")} (⇄)`, checked: pair.mode === "linked", run: () => A.setPairMode(pair.id, "linked") },
          { label: `${tr("pair.tandem")} (🔗)`, checked: pair.mode === "tandem", run: () => A.setPairMode(pair.id, "tandem") },
          { separator: true as const },
          {
            label: `${tr("pair.flowCopy")} (→)`,
            checked: pair.mode === "flow" && pair.flowDir === "leftToRight" && pair.flowOp === "copy",
            run: () => A.setPairMode(pair.id, "flow", "leftToRight", "copy"),
          },
          {
            label: `${tr("pair.flowMove")} (→)`,
            checked: pair.mode === "flow" && pair.flowDir === "leftToRight" && pair.flowOp === "move",
            run: () => A.setPairMode(pair.id, "flow", "leftToRight", "move"),
          },
          {
            label: `${tr("pair.flowCopy")} (←)`,
            checked: pair.mode === "flow" && pair.flowDir === "rightToLeft" && pair.flowOp === "copy",
            run: () => A.setPairMode(pair.id, "flow", "rightToLeft", "copy"),
          },
          {
            label: `${tr("pair.flowMove")} (←)`,
            checked: pair.mode === "flow" && pair.flowDir === "rightToLeft" && pair.flowOp === "move",
            run: () => A.setPairMode(pair.id, "flow", "rightToLeft", "move"),
          },
        ],
      },
      ...(pair.mode === "tandem" ? [{ label: tr("pair.reanchor"), run: () => A.reanchorPair(pair.id) }] : []),
      ...(pair.mode === "flow" ? [{ label: tr("pair.sendFlow"), shortcut: keys.action("pairFlowSend") || undefined, run: () => { void A.sendAcrossFlow(pair.id); } }] : []),
      { label: tr("pair.unpair"), run: () => A.unpairTabs(pair.id) },
    ];
  };

  const tabMenu = (e: React.MouseEvent, idx: number) => {
    e.preventDefault();
    const t = browser.tabs[idx];
    const pair = findPairForTab(session, b, t.id);
    A.showMenuFor(e, [
      { label: tr("tab.new"), shortcut: "Ctrl+T", run: () => A.sessionCmd({ op: "newTab", b }) },
      { label: tr("tab.duplicateOther"), run: () => A.sessionCmd({ op: "transfer", b, idx, copy: true }) },
      { label: tr("menu.moveToOther"), shortcut: "Alt+Middle-click", run: () => A.sessionCmd({ op: "transfer", b, idx, copy: false }) },
      ...(pairsEnabled ? [{ separator: true as const }, ...pairMenuItems(idx, pair)] : []),
      { separator: true },
      {
        label: tt("Change alias…"),
        run: async () => {
          const v = await A.promptText(tt("Tab alias"), t.alias ?? basename(t.path), "Save");
          if (v !== null) A.sessionCmd({ op: "alias", b, idx, alias: v, keep: t.keepAlias });
        },
      },
      { label: tr("tab.keepAlias"), checked: t.keepAlias, disabled: !t.alias, run: () => A.sessionCmd({ op: "alias", b, idx, alias: t.alias, keep: !t.keepAlias }) },
      { label: tr("sidebar.removeAlias"), disabled: !t.alias, run: () => A.sessionCmd({ op: "alias", b, idx, alias: null, keep: false }) },
      { separator: true },
      { label: tr("tab.close"), shortcut: "Ctrl+W", disabled: browser.tabs.length < 2, run: () => A.sessionCmd({ op: "close", b, idx }) },
      { label: tr("tab.closeOthers"), disabled: browser.tabs.length < 2, run: () => A.sessionCmd({ op: "closeOthers", b, idx }) },
      { label: tr("tab.closeStale"), run: () => A.sessionCmd({ op: "closeStale", b }) },
      { label: tr("tab.reopen"), shortcut: "Ctrl+Shift+T", disabled: !browser.closed.length, run: () => A.sessionCmd({ op: "reopen", b }) },
    ]);
  };

  return (
    <div
      className="tabbar"
      onWheel={(e) => {
        const n = browser.tabs.length;
        if (n < 2) return;
        const next = (browser.active + (e.deltaY > 0 ? 1 : -1) + n) % n;
        A.sessionCmd({ op: "activate", b, idx: next });
      }}
    >
      {browser.tabs.map((t, idx) => {
        const label = t.alias ?? (settings.tabsShowFullPath ? t.path : basename(t.path));
        const k = `t${t.id}`;
        const pair = pairsEnabled ? findPairForTab(session, b, t.id) : null;
        const partner = pair ? partnerTabOf(session, pair, b) : null;
        const pColor = pair ? pairColor(pair) : undefined;
        const isPairHovered = !!pair && hoveredPairId === pair.id;
        const tip = pair && partner
          ? `${t.path}\n${pairGlyph(pair)} ${pairModeLabel(pair.mode, pair.flowDir, pair.flowOp)} ↔ ${partner.tab.path}`
          : t.path;
        return (
          <div
            key={t.id}
            data-tab-id={t.id}
            data-pair-id={pair?.id}
            data-pair-mode={pair?.mode}
            className={`tab ${idx === browser.active ? "active" : ""} ${t.alias ? "alias" : ""} ${drop === k ? "drop" : ""} ${pairDrop === k ? "pair-drop" : ""} ${pair ? "paired" : ""} ${isPairHovered ? "pair-hover" : ""}`}
            style={pColor ? ({ "--pair-color": pColor } as React.CSSProperties) : undefined}
            title={tip}
            draggable
            onDragStart={(e) => startTabDrag(e, b, idx, t.path)}
            onMouseEnter={() => {
              armTimer.current = window.setTimeout(() => setArmed(t.id), 500);
              if (pair) A.setHoveredPair(pair.id);
            }}
            onMouseLeave={() => {
              if (armTimer.current) clearTimeout(armTimer.current);
              if (middlePressed.current === t.id) middlePressed.current = null;
              setArmed(null);
              if (pair && hoveredPairId === pair.id) A.setHoveredPair(null);
            }}
            onMouseDown={(e) => {
              middlePressed.current = e.button === 1 ? t.id : null;
              if (e.button === 1) {
                e.preventDefault();
                e.stopPropagation();
              }
            }}
            onMouseUp={(e) => {
              if (e.button === 1) {
                e.preventDefault();
                e.stopPropagation();
                const pressed = middlePressed.current;
                middlePressed.current = null;
                if (pressed !== t.id) return;
                if (e.altKey) A.sessionCmd({ op: "transfer", b, idx, copy: false });
                else A.sessionCmd({ op: "close", b, idx });
              }
            }}
            onAuxClick={(e) => {
              if (e.button === 1) {
                e.preventDefault();
                e.stopPropagation();
              }
            }}
            onClick={() => {
              A.focusBrowser(b);
              A.sessionCmd({ op: "activate", b, idx });
            }}
            onContextMenu={(e) => tabMenu(e, idx)}
            onDragOver={(e) => {
              const src = draggedTab();
              if (src) {
                e.preventDefault();
                if (src.b !== b && e.altKey) {
                  setPairDrop(k);
                  setDrop(null);
                } else {
                  setDrop(k);
                  setPairDrop(null);
                }
              } else if (isFileDrag(e) && onDragOverTarget(e, t.path)) setDrop(k);
            }}
            onDragLeave={() => {
              setDrop(null);
              setPairDrop(null);
            }}
            onDrop={(e) => {
              setDrop(null);
              setPairDrop(null);
              const src = draggedTab();
              if (src) {
                e.preventDefault();
                if (src.b === b) {
                  A.sessionCmd({ op: "reorder", b, from: src.idx, to: idx });
                } else if (e.altKey) {
                  const leftIdx = b === 0 ? idx : src.idx;
                  const rightIdx = b === 1 ? idx : src.idx;
                  A.pairTabs(leftIdx, rightIdx, "linked");
                } else {
                  A.sessionCmd({ op: "transfer", b: src.b, idx: src.idx, copy: false });
                }
                return;
              }
              dropInto(e, t.path);
            }}
          >
            <Icon name="folder" size={13} color="var(--folder)" />
            <span className="label">{label}</span>
            {pair && (
              <button
                type="button"
                className={`tab-pair-badge mode-${pair.mode}`}
                title={`${pairModeLabel(pair.mode, pair.flowDir, pair.flowOp)} — ${
                  pair.mode === "tandem"
                    ? tr("pair.descTandem")
                    : pair.mode === "flow"
                      ? tr("pair.descFlow")
                      : tr("pair.descLinked")
                }${partner ? `\n↔ ${tildify(partner.tab.path, home)}` : ""}\n${
                  singlePane && partner
                    ? tr("pair.switchToPartner", { name: basename(partner.tab.path) || partner.tab.path })
                    : tr("pair.clickCycle")
                }`}
                onClick={(e) => {
                  e.stopPropagation();
                  if (singlePane && partner) {
                    A.focusBrowser(partner.b);
                  } else {
                    A.cycleActivePairMode(pair.id);
                  }
                }}
              >
                {pairGlyph(pair)}
              </button>
            )}
            {browser.tabs.length > 1 && (
              <button
                className={`close ${armed === t.id || idx === browser.active ? "armed" : ""}`}
                title={tt("Close tab{value} — middle-click tab", { value: keys.action("closeTab") ? ` (${keys.action("closeTab")})` : "" })}
                onClick={(e) => {
                  e.stopPropagation();
                  A.sessionCmd({ op: "close", b, idx });
                }}
              >
                ×
              </button>
            )}
          </div>
        );
      })}
      {gravityEnabled && ghost && !singlePane && (
        <div
          className={`tab tab-ghost reason-${ghost.reason}`}
          data-ghost-path={ghost.path}
          title={`${
            ghost.reason === "worktree"
              ? tr("gravity.worktreeTip", { path: tildify(ghost.path, home) })
              : tr("gravity.learnedTip", { path: tildify(ghost.path, home) })
          }${keys.action("gravityAccept") ? ` (${platformLabel(keys.action("gravityAccept")!)})` : ""}`}
          onClick={(e) => {
            e.stopPropagation();
            void A.acceptGravityGhost(b, !e.shiftKey);
          }}
          onContextMenu={(e) => {
            e.preventDefault();
            e.stopPropagation();
            A.showMenuFor(e, [
              { label: tr("gravity.openAndPair"), shortcut: keys.action("gravityAccept") || undefined, run: () => { void A.acceptGravityGhost(b, true); } },
              { label: tr("gravity.openOnly"), run: () => { void A.acceptGravityGhost(b, false); } },
              { separator: true },
              { label: tr("gravity.dismiss"), run: () => { void A.dismissGravityGhost(b); } },
            ]);
          }}
        >
          <span className="ghost-magnet" aria-hidden>🧲</span>
          <span className="label">{basename(ghost.path) || ghost.path}</span>
          {ghost.reason === "worktree" && ghost.hint && (
            <span className="ghost-hint">{ghost.hint}</span>
          )}
          <button
            type="button"
            className="ghost-dismiss"
            title={`${tr("gravity.dismiss")}\n${tr("gravity.dismissHint")}`}
            aria-label={tr("gravity.dismiss")}
            onClick={(e) => {
              e.stopPropagation();
              void A.dismissGravityGhost(b);
            }}
          >
            ×
          </button>
        </div>
      )}
      <button
        className={`tab-add ${drop === "add" ? "drop" : ""}`}
        title={platformLabel(tt("New tab (Ctrl+T) — drop a tab here to duplicate it"))}
        onClick={() => A.sessionCmd({ op: "newTab", b })}
        onDragOver={(e) => {
          if (draggedTab()) {
            e.preventDefault();
            setDrop("add");
          }
        }}
        onDragLeave={() => setDrop(null)}
        onDrop={(e) => {
          setDrop(null);
          const src = draggedTab();
          if (!src) return;
          e.preventDefault();
          if (src.b === b) A.sessionCmd({ op: "newTab", b, path: browserPath(src) });
          else A.sessionCmd({ op: "transfer", b: src.b, idx: src.idx, copy: true });
        }}
      >
        <Icon name="plus" size={14} />
      </button>
      <div style={{ flex: 1 }} />
      {right}
    </div>
  );

  function browserPath(src: { idx: number }) {
    return browser.tabs[src.idx]?.path;
  }
}
