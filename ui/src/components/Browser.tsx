// Browser = tabs + navigation + folder pane + details, arranged per layout (W-02…W-06).
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { Layout } from "../api/types";
import type { Action } from "../core/keymap";
import { useKeyLabels } from "../state/keyLabels";
import * as A from "../state/actions";
import { useSize } from "../state/hooks";
import { getState, setState, useStore } from "../state/store";
import { DetailsPane } from "./DetailsPane";
import { FolderPane } from "./FolderPane";
import { Icon } from "./Icon";
import { Breadcrumbs, MillerColumns } from "./NavPane";
import { Splitter, useStoredSize } from "./Splitter";
import { TabBar } from "./TabBar";

function Browser({ b, layout }: { b: number; layout: Layout }) {
  const session = useStore((s) => s.session)!;
  const navColumns = useStore((s) => A.columnsShown(s, b));
  const detailsVisible = useStore((s) => A.detailsShown(s, b));
  const keys = useKeyLabels();
  const browser = session.browsers[b];
  const tab = browser.tabs[browser.active];
  // In Classic the active browser is the one shown (the other stays mounted, hidden).
  const active = session.activeBrowser === b;
  const [ref, size] = useSize<HTMLDivElement>();
  const [navH, setNavH] = useStoredSize(`nav-h-${layout}`, 150);
  const [detW, setDetW] = useStoredSize(`details-w-${layout}`, 300);
  const [dragStart, setDragStart] = useState(0);
  const [navHint, setNavHint] = useState(false);
  const navStart = useRef(0);

  const hint = (a: Action) => {
    const k = keys.action(a);
    return k ? ` (${k})` : "";
  };
  const navToggle = (
    <>
      {/* Every layout: Standard and Classic put the columns above the list, Columns
          shows the list as the last column; off is a breadcrumb bar in all three. */}
      <button className={`icon-btn ${navColumns ? "active" : ""}`}
        title={`${navColumns ? "Show breadcrumbs instead of columns" : "Show Miller columns"} in this browser${hint("toggleColumns")}`}
        onClick={() => A.toggleColumns(b)}>
        <Icon name={navColumns ? "columns" : "crumbs"} />
      </button>
      <button className={`icon-btn ${detailsVisible ? "active" : ""}`} title={`${detailsVisible ? "Hide" : "Show"} preview & details pane in this browser${hint("togglePreviewPane")}`}
        onClick={() => A.toggleDetails(b)}>
        <Icon name="pane" />
      </button>
    </>
  );

  const details = detailsVisible && size.w > 520 && (
    <>
      <Splitter dir="v" onDragStart={() => setDragStart(detW)} onDrag={(d) => setDetW(Math.max(180, Math.min(size.w * 0.6, dragStart - d)))} />
      <div style={{ flex: `0 0 ${detW}px`, display: "flex", minWidth: 0 }}>
        <DetailsPane b={b} path={tab.path} showToolbar={layout !== "classic"} />
      </div>
    </>
  );

  return (
    <div ref={ref} className={`browser ${active ? "active" : "inactive"}`} onMouseDown={() => A.focusBrowser(b)}>
      <TabBar b={b} browser={browser} right={navToggle} />
      {(layout === "standard" || layout === "classic" || !navColumns) && (
        <>
          {navColumns ? (
            <>
              <div style={{ height: navH, display: "flex", minHeight: 0, position: "relative" }}>
                <MillerColumns b={b} path={tab.path} width={size.w} includeCurrent />
                {navHint && <div className="collapse-hint">Release to switch to breadcrumbs</div>}
              </div>
              <Splitter
                dir="h"
                onDragStart={() => (navStart.current = navH)}
                onDrag={(d) => {
                  const h = navStart.current + d;
                  setNavHint(h < 70);
                  setNavH(Math.max(40, Math.min(size.h - 160, h)));
                }}
                onDragEnd={() => {
                  if (navHint) {
                    A.toggleColumns(b, false);
                    setNavH(150);
                  }
                  setNavHint(false);
                }}
              />
            </>
          ) : (
            <Breadcrumbs b={b} path={tab.path} />
          )}
          <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
            <FolderPane b={b} path={tab.path} active={active} />
            {details}
          </div>
        </>
      )}
      {layout === "columns" && navColumns && (
        <div style={{ flex: 1, display: "flex", minHeight: 0 }}>
          <MillerColumns b={b} path={tab.path} width={size.w - (detailsVisible ? detW : 0)} includeCurrent={false}
            trailing={<div style={{ flex: "1 0 320px", display: "flex", minWidth: 320 }}><FolderPane b={b} path={tab.path} active={active} compact /></div>} />
          {details}
        </div>
      )}

    </div>
  );
}

/** Two browsers with a collapsible divider, or one browser in Classic layout. */
export function Workspace() {
  const session = useStore((s) => s.session);
  const [ref, size] = useSize<HTMLDivElement>();
  const layout = session?.layout ?? "standard";
  const vertical = layout === "columns";
  const [ratio, setRatio] = useStoredSize(`split-${layout}`, 500);
  const [start, setStart] = useState(0);
  const [hint, setHint] = useState<0 | 1 | null>(null);
  // The title bar's panel controls sit right above the divider (the middle of the
  // workspace when one browser is shown). Where the divider is depends only on this
  // component's layout (its size, the ratio, the layout, collapsed browsers), so it is
  // measured after each of its renders: window and sidebar resizes, font changes and
  // splitter drags all re-render it.
  useLayoutEffect(() => {
    const ws = ref.current;
    const bar = document.querySelector<HTMLElement>(".titlebar");
    if (!ws || !bar) return;
    const r = (ws.querySelector<HTMLElement>(":scope > .ws-split.v") ?? ws).getBoundingClientRect();
    bar.style.setProperty("--pm-x", `${r.left + r.width / 2 - bar.getBoundingClientRect().left}px`);
  });
  // Which browsers are on screen, for actions on "the other browser" (none to act on
  // in Classic, with a collapsed browser or in a narrow window).
  const c0s = !!session?.browsers[0].collapsed;
  const c1s = !!session?.browsers[1].collapsed;
  const narrow = layout !== "columns" && size.w > 0 && size.w < 760;
  const oneShown = !session ? null : layout === "classic" || c0s || c1s || narrow
    ? (layout === "classic" ? session.activeBrowser : c0s ? 1 : c1s ? 0 : session.activeBrowser)
    : null;
  useEffect(() => {
    const shown = oneShown == null ? [0, 1] : [oneShown];
    if (getState().shownBrowsers.join() !== shown.join()) setState({ shownBrowsers: shown });
  }, [oneShown]);
  if (!session) return <div className="workspace" ref={ref} />;

  const total = vertical ? size.h : size.w;
  const minSize = vertical ? 160 : 300;
  const c0 = session.browsers[0].collapsed;
  const c1 = session.browsers[1].collapsed;
  // Auto-hide one browser if the window is too narrow (not persisted).
  const tooNarrow = !vertical && size.w > 0 && size.w < 760;
  const single = layout === "classic" || c0 || c1 || tooNarrow;
  const only = layout === "classic" ? session.activeBrowser : c0 ? 1 : c1 ? 0 : session.activeBrowser;

  // Both browsers stay in the same place in the tree, the hidden one with display:none:
  // switching between one and two browsers (Classic, ‹ / ›, a narrow window) must not
  // re-create them, which lost the selection, the filter and the scroll position.
  const restore = single && (c0 || c1) && layout !== "classic" && !tooNarrow;
  const first = Math.max(40, Math.min(total - 40, (ratio / 1000) * total));
  const box = (b: 0 | 1): React.CSSProperties =>
    single && only !== b ? { display: "none" }
    : { flex: single || b === 1 ? 1 : `0 0 ${first}px`, display: "flex", minWidth: 0, minHeight: 0, position: "relative" };
  return (
    <div className={`workspace ${vertical ? "vertical" : ""}`} ref={ref}>
      {restore && c0 && <RestoreStrip vertical={vertical} onClick={() => A.sessionCmd({ op: "collapse", b: 0, collapsed: false })} />}
      <div style={box(0)}>
        <Browser b={0} layout={layout} />
        {hint === 0 && <div className="collapse-hint">Release to collapse this browser</div>}
      </div>
      {!single && <Splitter
        className="ws-split"
        dir={vertical ? "h" : "v"}
        onReset={() => setRatio(500)}
        onDragStart={() => setStart(first)}
        onDrag={(d) => {
          const px = start + d;
          setHint(px < minSize ? 0 : total - px < minSize ? 1 : null);
          setRatio((Math.max(40, Math.min(total - 40, px)) / total) * 1000);
        }}
        onDragEnd={() => {
          if (hint !== null) {
            A.sessionCmd({ op: "collapse", b: hint, collapsed: true });
            setRatio(500);
          }
          setHint(null);
        }}
      />}
      <div style={box(1)}>
        <Browser b={1} layout={layout} />
        {hint === 1 && <div className="collapse-hint">Release to collapse this browser</div>}
      </div>
      {restore && c1 && <RestoreStrip vertical={vertical} onClick={() => A.sessionCmd({ op: "collapse", b: 1, collapsed: false })} />}
    </div>
  );
}

function RestoreStrip({ vertical, onClick }: { vertical: boolean; onClick: () => void }) {
  return (
    <button
      onClick={onClick}
      title="Show second browser"
      style={{
        flex: "0 0 14px", border: "none", background: "var(--bg)", cursor: "pointer", color: "var(--fg-dim)",
        borderLeft: vertical ? "none" : "1px solid var(--border)", borderTop: vertical ? "1px solid var(--border)" : "none",
      }}
    >
      {vertical ? "˄" : "‹"}
    </button>
  );
}
