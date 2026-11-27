// Minimal numbered workspace switcher (`1 2 3 +`) in the title bar with Drop Portal (WN-179).
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { WorkspaceSummary } from "../api/types";
import { platformLabel, type Action } from "../core/keymap";
import {
  DROP_PORTAL_DELAY_MS,
  MAX_WORKSPACES,
  WORKSPACE_ACCENTS,
  resolveDropPortalTargets,
  workspacePillLabel,
  workspaceTooltip,
} from "../core/workspaces";
import { t, tt } from "../i18n";
import * as A from "../state/actions";
import {
  draggedWorkspace,
  dropInto,
  isFileDrag,
  onDragOverTarget,
  startWorkspaceDrag,
  subscribeDragEnd,
} from "../state/dnd";
import { useKeyLabels } from "../state/keyLabels";
import { useStore } from "../state/store";
import { Icon } from "./Icon";

interface PortalState {
  workspace: WorkspaceSummary;
  x: number;
  y: number;
}

export function WorkspaceSwitcher() {
  const workspaces = useStore((s) => s.workspaces);
  const active = useStore((s) => s.activeWorkspace);
  const showWorkspaces = useStore((s) => s.settings?.showWorkspaces ?? "always");
  const pick = useStore((s) => s.pick);
  const home = useStore((s) => s.info?.home ?? "");
  const keys = useKeyLabels();

  const [reorderOver, setReorderOver] = useState<number | null>(null);
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  const [portal, setPortal] = useState<PortalState | null>(null);
  const [portalHoverSide, setPortalHoverSide] = useState<number | null>(null);

  const openTimer = useRef<number | null>(null);
  const closeTimer = useRef<number | null>(null);
  const middlePressed = useRef<number | null>(null);

  const clearTimers = () => {
    if (openTimer.current !== null) {
      window.clearTimeout(openTimer.current);
      openTimer.current = null;
    }
    if (closeTimer.current !== null) {
      window.clearTimeout(closeTimer.current);
      closeTimer.current = null;
    }
  };

  const closePortalNow = () => {
    clearTimers();
    setHoverIdx(null);
    setPortal(null);
    setPortalHoverSide(null);
  };

  const scheduleClosePortal = () => {
    if (openTimer.current !== null) {
      window.clearTimeout(openTimer.current);
      openTimer.current = null;
    }
    if (closeTimer.current !== null) window.clearTimeout(closeTimer.current);
    closeTimer.current = window.setTimeout(() => {
      closeTimer.current = null;
      setHoverIdx(null);
      setPortal(null);
      setPortalHoverSide(null);
    }, 180);
  };

  useEffect(() => {
    const off = subscribeDragEnd(closePortalNow);
    return () => {
      off();
      clearTimers();
    };
  }, []);

  if (pick || !workspaces.length) return null;
  if (showWorkspaces === "never") return null;
  if (showWorkspaces === "multiple" && workspaces.length < 2) return null;

  const activeWs = workspaces[active];
  const activeId = activeWs?.id ?? null;

  const openMenu = (e: React.MouseEvent, ws: WorkspaceSummary, idx: number) => {
    e.preventDefault();
    e.stopPropagation();
    const canClose = workspaces.length > 1;
    const canAdd = workspaces.length < MAX_WORKSPACES;
    A.showMenuFor(e, [
      { label: t("workspace.rename"), run: () => { void A.promptRenameWorkspace(idx); } },
      {
        label: t("workspace.accent"),
        submenu: [
          {
            label: t("workspace.accentNone"),
            checked: !ws.accent,
            run: () => A.setWorkspaceAccent(idx, null),
          },
          { separator: true },
          ...WORKSPACE_ACCENTS.map((a) => ({
            label: tt(a.label),
            swatch: a.hex,
            checked: ws.accent?.toLowerCase() === a.hex,
            run: () => A.setWorkspaceAccent(idx, a.hex),
          })),
        ],
      },
      { separator: true },
      { label: t("workspace.duplicate"), disabled: !canAdd, run: () => A.duplicateWorkspace(idx) },
      { label: t("workspace.new"), disabled: !canAdd, run: () => A.createWorkspace() },
      { separator: true },
      { label: t("workspace.moveLeft"), disabled: idx === 0, run: () => A.reorderWorkspace(idx, idx - 1) },
      { label: t("workspace.moveRight"), disabled: idx >= workspaces.length - 1, run: () => A.reorderWorkspace(idx, idx + 1) },
      { separator: true },
      { label: t("workspace.close"), disabled: !canClose, danger: true, run: () => A.closeWorkspace(idx) },
    ]);
  };

  const handlePillDragOver = (e: React.DragEvent<HTMLButtonElement>, ws: WorkspaceSummary, idx: number) => {
    const wsDrag = draggedWorkspace();
    if (wsDrag) {
      e.preventDefault();
      if (wsDrag.idx !== idx) setReorderOver(idx);
      return;
    }
    if (!isFileDrag(e)) return;
    if (ws.id === activeId) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
    if (closeTimer.current !== null) {
      window.clearTimeout(closeTimer.current);
      closeTimer.current = null;
    }
    setHoverIdx(idx);
    if (portal?.workspace.id === ws.id) return;
    if (openTimer.current === null) {
      const rect = e.currentTarget.getBoundingClientRect();
      const x = Math.max(12, Math.min(window.innerWidth - 340, rect.left));
      const y = rect.bottom + 6;
      openTimer.current = window.setTimeout(() => {
        openTimer.current = null;
        setPortal({ workspace: ws, x, y });
      }, DROP_PORTAL_DELAY_MS);
    }
  };

  const handlePillDrop = (e: React.DragEvent<HTMLButtonElement>, ws: WorkspaceSummary, idx: number) => {
    const wsDrag = draggedWorkspace();
    if (wsDrag) {
      e.preventDefault();
      setReorderOver(null);
      if (wsDrag.idx !== idx) A.reorderWorkspace(wsDrag.idx, idx);
      return;
    }
    if (isFileDrag(e) && ws.id !== activeId) {
      e.preventDefault();
      // Dropped directly onto the pill: route into that workspace's active pane without switching context.
      const dest = ws.activeBrowser === 1 ? ws.rightPath : ws.leftPath;
      closePortalNow();
      dropInto(e, dest);
    }
  };

  const portalTargets = portal ? resolveDropPortalTargets(portal.workspace, activeId, home) : null;
  const newShortcut = keys.action("workspaceNew");

  return (
    <>
      <div className="ws-bar" role="tablist" aria-label={t("workspace.label")}>
        {workspaces.map((ws, idx) => {
          const isCurrent = idx === active;
          const { num, name } = workspacePillLabel(ws, idx);
          const action = idx < 9 ? (`workspace${idx + 1}` as Action) : null;
          const shortcut = action ? keys.action(action) : "";
          const wsHint = isCurrent ? t("workspace.pillHintActive") : t("workspace.pillHintInactive");
          const tip = workspaceTooltip(ws, idx, home, shortcut ? platformLabel(shortcut) : undefined, wsHint);
          return (
            <button
              key={ws.id}
              type="button"
              role="tab"
              aria-selected={isCurrent}
              data-ws-id={ws.id}
              data-ws-index={idx}
              className={`ws-pill${isCurrent ? " active" : ""}${reorderOver === idx ? " drop-reorder" : ""}${hoverIdx === idx ? " portal-armed" : ""}${ws.accent ? " tinted" : ""}`}
              style={ws.accent ? ({ "--ws-pill-accent": ws.accent } as React.CSSProperties) : undefined}
              title={tip}
              draggable
              onClick={() => A.switchWorkspace(idx)}
              onDoubleClick={(e) => {
                e.stopPropagation();
                void A.promptRenameWorkspace(idx);
              }}
              onContextMenu={(e) => openMenu(e, ws, idx)}
              onMouseDown={(e) => {
                middlePressed.current = e.button === 1 ? ws.id : null;
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
                  if (pressed === ws.id && workspaces.length > 1) A.closeWorkspace(idx);
                }
              }}
              onAuxClick={(e) => {
                if (e.button === 1) {
                  e.preventDefault();
                  e.stopPropagation();
                }
              }}
              onDragStart={(e) => startWorkspaceDrag(e, idx, name ? `${num} ${name}` : num)}
              onDragOver={(e) => handlePillDragOver(e, ws, idx)}
              onDragLeave={() => {
                setReorderOver((prev) => (prev === idx ? null : prev));
                if (hoverIdx === idx) scheduleClosePortal();
              }}
              onDrop={(e) => handlePillDrop(e, ws, idx)}
            >
              {ws.accent && <span className="ws-dot" aria-hidden />}
              <span className="ws-num">{num}</span>
              {name && <span className="ws-name">{name}</span>}
              {ws.pairCount > 0 && <span className="ws-pairs-dot" aria-hidden />}
            </button>
          );
        })}
        {workspaces.length < MAX_WORKSPACES && (
          <button
            type="button"
            className="ws-add"
            title={`${newShortcut ? `${t("workspace.new")} (${platformLabel(newShortcut)})` : t("workspace.new")}\n${t("workspace.newHint")}`}
            aria-label={t("workspace.new")}
            onClick={() => A.createWorkspace()}
          >
            <Icon name="plus" size={12} />
          </button>
        )}
      </div>

      {portal && portalTargets && createPortal(
        <div
          className="ws-drop-portal"
          role="dialog"
          aria-label={t("workspace.dropPortalTitle", { name: portal.workspace.name || String(portal.workspace.index + 1) })}
          style={{
            left: portal.x,
            top: portal.y,
            ...(portal.workspace.accent ? ({ "--ws-portal-accent": portal.workspace.accent } as React.CSSProperties) : {}),
          }}
          onDragOver={(e) => {
            if (!isFileDrag(e)) return;
            e.preventDefault();
            if (closeTimer.current !== null) {
              window.clearTimeout(closeTimer.current);
              closeTimer.current = null;
            }
          }}
          onDragLeave={(e) => {
            if (!e.currentTarget.contains(e.relatedTarget as Node | null)) {
              scheduleClosePortal();
            }
          }}
        >
          <div className="ws-portal-head">
            <span className="ws-portal-badge">
              {portal.workspace.accent && <span className="ws-dot" aria-hidden />}
              {portal.workspace.index + 1}
            </span>
            <span className="ws-portal-title">
              {t("workspace.dropPortalTitle", {
                name: portal.workspace.name || `${t("workspace.single")} ${portal.workspace.index + 1}`,
              })}
            </span>
          </div>
          <div className={`ws-portal-targets${portalTargets.length === 1 ? " single" : ""}`}>
            {portalTargets.map((target) => (
              <div
                key={target.side}
                className={`ws-portal-target${portalHoverSide === target.side ? " hover" : ""}`}
                data-portal-side={target.side}
                title={`${target.path}\n${t("workspace.dropTargetHint")}`}
                onDragOver={(e) => {
                  if (closeTimer.current !== null) {
                    window.clearTimeout(closeTimer.current);
                    closeTimer.current = null;
                  }
                  if (isFileDrag(e) && onDragOverTarget(e, target.path)) {
                    setPortalHoverSide(target.side);
                  }
                }}
                onDragLeave={() => setPortalHoverSide((prev) => (prev === target.side ? null : prev))}
                onDrop={(e) => {
                  closePortalNow();
                  dropInto(e, target.path);
                }}
              >
                <div className="ws-portal-side">{tt(target.sideLabel)}</div>
                <div className="ws-portal-folder">
                  <Icon name="folder" size={14} color="var(--folder)" />
                  <span className="ws-portal-name">{target.name}</span>
                </div>
                {target.parentPath && <div className="ws-portal-path">{target.parentPath}</div>}
              </div>
            ))}
          </div>
        </div>,
        document.body,
      )}
    </>
  );
}
