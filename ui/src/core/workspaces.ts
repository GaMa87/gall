// Pure helpers for numbered workspaces (`1 2 3 +`) and Drop Portal target resolution (WN-179).
import type { Session, WorkspaceSummary } from "../api/types";
import { basename, parent, tildify } from "./path";

export const MAX_WORKSPACES = 9;
export const DROP_PORTAL_DELAY_MS = 250;

/** Curated subtle accent tints for workspaces (aligned with Gall's tag palette). */
export const WORKSPACE_ACCENTS: { hex: string; label: string }[] = [
  { hex: "#3d8bfd", label: "Blue" },
  { hex: "#3fb27f", label: "Green" },
  { hex: "#e8913a", label: "Amber" },
  { hex: "#a371f7", label: "Purple" },
  { hex: "#db61a2", label: "Pink" },
  { hex: "#2bbac5", label: "Teal" },
  { hex: "#e2c541", label: "Gold" },
  { hex: "#e5534b", label: "Red" },
];

export interface DropPortalTarget {
  side: 0 | 1;
  sideLabel: "Left" | "Right" | "Top" | "Bottom" | "Active";
  path: string;
  name: string;
  parentPath: string;
}

/** Compact label for a workspace pill: `1` when unnamed, `1 · Dev` when named. */
export function workspacePillLabel(ws: Pick<WorkspaceSummary, "name">, index: number): { num: string; name: string | null } {
  const trimmed = ws.name?.trim() || null;
  return { num: String(index + 1), name: trimmed };
}

/** Tooltip summarising the workspace name, shortcut, active left/right paths, and interaction hint. */
export function workspaceTooltip(
  ws: WorkspaceSummary,
  index: number,
  home: string,
  shortcut?: string,
  hint?: string,
): string {
  const title = ws.name ? `${index + 1}. ${ws.name}` : `Workspace ${index + 1}`;
  const head = shortcut ? `${title} (${shortcut})` : title;
  const left = tildify(ws.leftPath, home);
  const right = tildify(ws.rightPath, home);
  const paths = ws.layout === "classic"
    ? (ws.activeBrowser === 1 ? right : left)
    : `${left}  ⇄  ${right}`;
  return hint ? `${head}\n${paths}\n${hint}` : `${head}\n${paths}`;
}

/**
 * Resolves the drop targets displayed in the Drop Portal popover when dragging files
 * over an inactive workspace pill. Returns `null` for the currently active workspace.
 */
export function resolveDropPortalTargets(
  ws: WorkspaceSummary,
  activeWorkspaceId: number | null,
  home = "",
): DropPortalTarget[] | null {
  if (activeWorkspaceId !== null && ws.id === activeWorkspaceId) return null;
  const make = (side: 0 | 1, sideLabel: DropPortalTarget["sideLabel"], path: string): DropPortalTarget => {
    const p = parent(path);
    return {
      side,
      sideLabel,
      path,
      name: basename(path) || path,
      parentPath: p ? tildify(p, home) : "",
    };
  };
  if (ws.layout === "classic") {
    const side: 0 | 1 = ws.activeBrowser === 1 ? 1 : 0;
    const path = side === 1 ? ws.rightPath : ws.leftPath;
    return [make(side, "Active", path)];
  }
  const vertical = ws.layout === "columns";
  return [
    make(0, vertical ? "Top" : "Left", ws.leftPath),
    make(1, vertical ? "Bottom" : "Right", ws.rightPath),
  ];
}

/** Keeps the active workspace's lightweight summary in sync after any `sessionCmd`. */
export function syncActiveWorkspaceSummary(
  workspaces: WorkspaceSummary[],
  activeIndex: number,
  session: Session,
): WorkspaceSummary[] {
  if (!workspaces.length || activeIndex < 0 || activeIndex >= workspaces.length) return workspaces;
  const cur = workspaces[activeIndex];
  const leftPath = session.browsers[0]?.tabs[session.browsers[0].active]?.path ?? cur.leftPath;
  const rightPath = session.browsers[1]?.tabs[session.browsers[1].active]?.path ?? cur.rightPath;
  const leftTabs = session.browsers[0]?.tabs.length ?? cur.leftTabs;
  const rightTabs = session.browsers[1]?.tabs.length ?? cur.rightTabs;
  const pairCount = (session as { pairs?: unknown[] }).pairs?.length ?? cur.pairCount;
  if (
    cur.leftPath === leftPath &&
    cur.rightPath === rightPath &&
    cur.leftTabs === leftTabs &&
    cur.rightTabs === rightTabs &&
    cur.layout === session.layout &&
    cur.activeBrowser === session.activeBrowser &&
    cur.pairCount === pairCount
  ) {
    return workspaces;
  }
  const next = workspaces.slice();
  next[activeIndex] = {
    ...cur,
    layout: session.layout,
    activeBrowser: session.activeBrowser,
    leftPath,
    rightPath,
    leftTabs,
    rightTabs,
    pairCount,
  };
  return next;
}
