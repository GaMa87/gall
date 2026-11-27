// Positioned document comparison (.docx and .pdf): shows page/section and block markers,
// layout changes (heading-level shifts, moved blocks with partner jump, table-row edits),
// extraction warnings, immediate cancellation on close, and one-click text-diff fallback.
import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/ipc";
import type { DocBlock, DocExtract } from "../api/types";
import {
  type DocCompareRow,
  compareDocumentBlocks,
  formatBlockGeometry,
  formatBlockPosition,
} from "../core/documentBlocks";
import { basename } from "../core/path";
import { tt } from "../i18n";
import * as A from "../state/actions";
import { getState } from "../state/store";
import { Icon } from "./Icon";

let nextDocTicket = Date.now();

function layoutBadgeLabel(row: DocCompareRow): string | null {
  switch (row.layoutChange) {
    case "headingLevel":
      return `${tt("Heading")} ${row.layoutDetail ?? ""}`.trim();
    case "kindChanged":
      return `${tt("Layout")} ${row.layoutDetail ?? ""}`.trim();
    case "movedFrom":
      return `${tt("Moved")} ${row.layoutDetail ?? ""}`.trim();
    case "movedTo":
      return `${tt("Moved")} ${row.layoutDetail ?? ""}`.trim();
    case "tableRowAdded":
      return tt("Table row added");
    case "tableRowRemoved":
      return tt("Table row removed");
    case "tableRowChanged":
      return `${tt("Table row")} · ${row.layoutDetail ?? ""}`.trim();
    default:
      return null;
  }
}

function renderBlockBody(block: DocBlock | null, changedCells: number[]) {
  if (!block) return <span className="doc-empty">—</span>;
  if (block.kind === "tableRow" && block.cells?.length) {
    const changedSet = changedCells.length ? new Set(changedCells) : null;
    return (
      <div className="doc-cells">
        {block.cells.map((cell, ci) => (
          <span key={ci} className={`doc-cell ${changedSet?.has(ci) ? "chg" : ""}`}>
            {cell || " "}
          </span>
        ))}
      </div>
    );
  }
  if (block.kind === "heading") {
    const lvl = block.headingLevel ?? 1;
    return (
      <span className={`doc-text doc-h doc-h${Math.min(4, lvl)}`}>
        <b className="doc-h-tag">H{lvl}</b> {block.text}
      </span>
    );
  }
  if (block.kind === "listItem") {
    return <span className="doc-text">• {block.text}</span>;
  }
  return <span className="doc-text">{block.text}</span>;
}

export function DocumentCompare({
  left,
  right,
  onText,
  onBinaryFallback,
  onCancelClose,
}: {
  left: string;
  right: string;
  onText: (leftDoc: DocExtract, rightDoc: DocExtract) => void;
  onBinaryFallback: () => void;
  onCancelClose: () => void;
}) {
  const [docs, setDocs] = useState<[DocExtract, DocExtract] | null>(null);
  const [loading, setLoading] = useState(true);
  const [onlyChanges, setOnlyChanges] = useState(false);
  const [cur, setCur] = useState(-1);
  const rowRefs = useRef(new Map<number, HTMLDivElement>());
  const ticketsRef = useRef<[number, number] | null>(null);

  useEffect(() => {
    let alive = true;
    const tickets: [number, number] = [++nextDocTicket, ++nextDocTicket];
    ticketsRef.current = tickets;
    setDocs(null);
    setLoading(true);
    setCur(-1);
    Promise.all([api.docExtract(left, tickets[0]), api.docExtract(right, tickets[1])])
      .then(([l, r]) => {
        if (!alive) return;
        ticketsRef.current = null;
        setDocs([l, r]);
        setLoading(false);
      })
      .catch((e) => {
        for (const t of tickets) void api.docExtractCancel(t).catch(() => {});
        if (!alive) return;
        ticketsRef.current = null;
        setLoading(false);
        const reason = String(e instanceof Error ? e.message : e);
        if (reason.toLowerCase().includes("cancelled")) return;
        A.notify(
          tt("Could not read the documents as text: {reason}. Comparing the files byte for byte instead.", { reason }),
          "warn",
        );
        onBinaryFallback();
      });
    return () => {
      alive = false;
      for (const t of tickets) void api.docExtractCancel(t).catch(() => {});
      ticketsRef.current = null;
    };
  }, [left, right]); // eslint-disable-line react-hooks/exhaustive-deps

  const cancelNow = () => {
    const active = ticketsRef.current;
    if (active) {
      for (const t of active) void api.docExtractCancel(t).catch(() => {});
      ticketsRef.current = null;
    }
    onCancelClose();
  };

  const compared = useMemo(() => (docs ? compareDocumentBlocks(docs[0], docs[1]) : null), [docs]);
  const visibleRows = useMemo(
    () => (compared ? (onlyChanges ? compared.rows.filter((r) => r.status !== "same") : compared.rows) : []),
    [compared, onlyChanges],
  );
  const changeIndices = useMemo(
    () => visibleRows.map((r, i) => (r.status !== "same" ? i : -1)).filter((i) => i >= 0),
    [visibleRows],
  );

  const jumpToRowIndex = (fullRowIndex: number) => {
    if (onlyChanges) {
      const pos = visibleRows.findIndex((r) => r.index === fullRowIndex);
      if (pos >= 0) {
        setCur(pos);
        rowRefs.current.get(fullRowIndex)?.scrollIntoView({ block: "center", behavior: "smooth" });
        return;
      }
      setOnlyChanges(false);
    }
    setCur(fullRowIndex);
    setTimeout(() => {
      rowRefs.current.get(fullRowIndex)?.scrollIntoView({ block: "center", behavior: "smooth" });
    }, 0);
  };

  const step = (dir: 1 | -1) => {
    if (!changeIndices.length) return;
    const pos = changeIndices.findIndex((i) => (dir > 0 ? i > cur : i >= cur));
    const k = dir > 0 ? (pos < 0 ? 0 : pos) : (pos <= 0 ? changeIndices.length - 1 : pos - 1);
    const nextVis = changeIndices[k];
    setCur(nextVis);
    const targetRow = visibleRows[nextVis];
    if (targetRow) rowRefs.current.get(targetRow.index)?.scrollIntoView({ block: "center" });
  };
  const stepRef = useRef(step);
  stepRef.current = step;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (getState().overlay?.type === "dialog") return;
      if ((e.target as HTMLElement)?.tagName === "INPUT") return;
      if (e.key === "n" || e.key === "]") stepRef.current(1);
      else if (e.key === "N" || e.key === "[") stepRef.current(-1);
      else if (e.key === "c") setOnlyChanges((v) => !v);
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  const warnings = useMemo(() => {
    if (!docs) return [];
    const list: string[] = [];
    for (const [path, doc] of [[left, docs[0]], [right, docs[1]]] as const) {
      const name = basename(path);
      for (const w of doc.warnings) list.push(`${name}: ${w}`);
    }
    return list;
  }, [docs, left, right]);

  if (loading && !docs) {
    return (
      <div className="doccmp">
        <div className="dv-msg">
          <span>{tt("Reading documents…")}</span>
          <button className="dc-btn" onClick={cancelNow}>{tt("Cancel")}</button>
        </div>
      </div>
    );
  }
  if (!docs || !compared) return null;

  const { counts, format } = compared;
  const totalChanges = counts.added + counts.removed + counts.changed + counts.moved;

  return (
    <div className="doccmp">
      <div className="doc-bar">
        <div className="doc-summary">
          <span className="add">+{counts.added}</span>
          <span className="del">−{counts.removed}</span>
          <span className="chg">~{counts.changed}</span>
          {counts.moved > 0 && <span className="mov">⇅ {counts.moved} {tt("moved")}</span>}
          {counts.headingChanges > 0 && <span className="lay">H {counts.headingChanges}</span>}
          {counts.tableRowChanges > 0 && <span className="lay">{tt("Table")} {counts.tableRowChanges}</span>}
          <span className="same">{counts.same} {tt("unchanged")}</span>
        </div>
        <button className={`dc-chip ${onlyChanges ? "on" : ""}`} title={tt("Show only changed blocks (c)")} onClick={() => setOnlyChanges((v) => !v)}>
          {tt("Changes only")}
        </button>
        <span className="spacer" />
        {changeIndices.length > 0 && (
          <div className="dv-nav">
            <button className="icon-btn" title={tt("Previous change (N)")} onClick={() => step(-1)}><Icon name="up" size={15} /></button>
            <span className="dv-count">{cur >= 0 ? changeIndices.indexOf(cur) + 1 : "–"} / {changeIndices.length}</span>
            <button className="icon-btn" title={tt("Next change (n)")} onClick={() => step(1)}><Icon name="down" size={15} /></button>
          </div>
        )}
        <button className="dc-btn" title={tt("Compare extracted document text line by line")} onClick={() => onText(docs[0], docs[1])}>
          {tt("Text view")}
        </button>
      </div>

      {warnings.length > 0 && (
        <div className="doc-warnings" role="status">
          {warnings.map((w, i) => <div key={i} className="doc-warn-line"><Icon name="compare" size={14} /> {w}</div>)}
        </div>
      )}

      {visibleRows.length === 0 ? (
        <div className="dv-msg same">
          <Icon name="check" size={16} />
          {totalChanges === 0 ? tt("No document differences") : tt("No matching blocks")}
        </div>
      ) : (
        <div className="doc-body">
          {visibleRows.map((row, visIdx) => {
            const badge = layoutBadgeLabel(row);
            const leftPos = row.left ? formatBlockPosition(row.left, format) : null;
            const rightPos = row.right ? formatBlockPosition(row.right, format) : null;
            const leftGeom = row.left ? formatBlockGeometry(row.left) : null;
            const rightGeom = row.right ? formatBlockGeometry(row.right) : null;
            return (
              <div
                key={row.index}
                ref={(el) => {
                  if (el) rowRefs.current.set(row.index, el);
                  else rowRefs.current.delete(row.index);
                }}
                className={`doc-row ${row.status} ${visIdx === cur ? "cur" : ""}`}
                onClick={() => setCur(visIdx)}
              >
                <div className="doc-side l">
                  <span className="doc-pos" title={leftGeom ?? undefined}>{leftPos ?? ""}</span>
                  <div className="doc-content">{renderBlockBody(row.left, row.changedCells)}</div>
                </div>
                <div className="doc-mid">
                  {badge && (
                    row.movePartnerIndex != null ? (
                      <button
                        className="doc-badge move"
                        title={tt("Jump to moved block")}
                        onClick={(e) => { e.stopPropagation(); jumpToRowIndex(row.movePartnerIndex!); }}
                      >
                        {badge}
                      </button>
                    ) : (
                      <span className={`doc-badge ${row.layoutChange ?? ""}`}>{badge}</span>
                    )
                  )}
                </div>
                <div className="doc-side r">
                  <span className="doc-pos" title={rightGeom ?? undefined}>{rightPos ?? ""}</span>
                  <div className="doc-content">{renderBlockBody(row.right, row.changedCells)}</div>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
