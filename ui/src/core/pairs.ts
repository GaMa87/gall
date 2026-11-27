// Pure helpers for Live Tab Pairs: Linked (⇄), Tandem (🔗), and Flow (→ / ←) (WN-180).
import type { FlowDirection, FlowOp, PairMode, Session, TabPair } from "../api/types";
import { basename, parent, tildify } from "./path";

/** Six harmonious pair colors so multiple live pairs in one workspace are distinct at a glance. */
export const PAIR_COLORS = [
  "#3d8bfd", // blue
  "#3fb27f", // green
  "#a371f7", // purple
  "#e8913a", // amber
  "#2bbac5", // teal
  "#db61a2", // pink
] as const;

export function pairColor(pair: Pick<TabPair, "colorSlot">): string {
  return PAIR_COLORS[pair.colorSlot % PAIR_COLORS.length];
}

export function pairGlyph(pair: Pick<TabPair, "mode" | "flowDir">): string {
  if (pair.mode === "tandem") return "🔗";
  if (pair.mode === "flow") return pair.flowDir === "rightToLeft" ? "←" : "→";
  return "⇄";
}

export function pairModeLabel(mode: PairMode, flowDir: FlowDirection = "leftToRight", flowOp: FlowOp = "copy"): string {
  if (mode === "tandem") return "Tandem walk";
  if (mode === "flow") {
    const verb = flowOp === "move" ? "Move" : "Copy";
    const arrow = flowDir === "rightToLeft" ? "Right → Left" : "Left → Right";
    return `Flow (${verb} ${arrow})`;
  }
  return "Linked";
}

export function nextPairMode(mode: PairMode): PairMode {
  if (mode === "linked") return "tandem";
  if (mode === "tandem") return "flow";
  return "linked";
}

/** Finds the pair a specific tab belongs to, if any. */
export function findPairForTab(session: Session | null | undefined, b: number, tabId: number): TabPair | null {
  if (!session?.pairs?.length) return null;
  return session.pairs.find((p) => (b === 0 ? p.leftTabId === tabId : p.rightTabId === tabId)) ?? null;
}

/** Finds the pair linking the currently active left and right tabs, if both belong to the same pair. */
export function activePair(session: Session | null | undefined): TabPair | null {
  if (!session?.pairs?.length || session.browsers.length < 2) return null;
  const left = session.browsers[0].tabs[session.browsers[0].active];
  const right = session.browsers[1].tabs[session.browsers[1].active];
  if (!left || !right) return null;
  return session.pairs.find((p) => p.leftTabId === left.id && p.rightTabId === right.id) ?? null;
}

/** Returns the partner browser index (`1 - b`) and tab for a pair. */
export function partnerTabOf(session: Session | null | undefined, pair: TabPair, b: number) {
  if (!session || session.browsers.length < 2) return null;
  const ob = b === 0 ? 1 : 0;
  const partnerId = b === 0 ? pair.rightTabId : pair.leftTabId;
  const idx = session.browsers[ob].tabs.findIndex((t) => t.id === partnerId);
  if (idx < 0) return null;
  return { b: ob as 0 | 1, idx, tab: session.browsers[ob].tabs[idx] };
}

/** Resolves Flow dispatch source/dest browsers and operation for a Flow pair. */
export function resolveFlowDispatch(
  session: Session | null | undefined,
  pair: TabPair | null | undefined,
): { srcBrowser: 0 | 1; dstBrowser: 0 | 1; dstPath: string; op: FlowOp } | null {
  if (!session || !pair || pair.mode !== "flow") return null;
  const srcBrowser: 0 | 1 = pair.flowDir === "rightToLeft" ? 1 : 0;
  const dstBrowser: 0 | 1 = srcBrowser === 0 ? 1 : 0;
  const dstTab = session.browsers[dstBrowser]?.tabs.find(
    (t) => t.id === (dstBrowser === 0 ? pair.leftTabId : pair.rightTabId),
  );
  if (!dstTab) return null;
  return { srcBrowser, dstBrowser, dstPath: dstTab.path, op: pair.flowOp };
}

/** Whether the Flow source tab is the tab currently controlled by the source pane. */
export function flowSourceTabIsActive(session: Session, pair: TabPair): boolean {
  const srcBrowser: 0 | 1 = pair.flowDir === "rightToLeft" ? 1 : 0;
  const sourceTabId = srcBrowser === 0 ? pair.leftTabId : pair.rightTabId;
  const browser = session.browsers[srcBrowser];
  return browser?.tabs[browser.active]?.id === sourceTabId;
}

/** Computes a compact relative subpath display for a Tandem ghost missing folder. */
export function tandemRelativeDisplay(path: string, root: string, home = ""): string {
  const norm = (s: string) => s.replace(/\\/g, "/").replace(/\/+$/, "");
  const p = norm(path);
  const r = norm(root);
  if (r && p.startsWith(r + "/")) return p.slice(r.length + 1);
  return tildify(path, home) || basename(path);
}

/** Finds the nearest parent path above `missingPath` (for Tandem "Open parent" recovery). */
export function tandemFallbackParent(missingPath: string, root: string): string {
  const p = parent(missingPath);
  return p && p !== missingPath ? p : root || "/";
}
