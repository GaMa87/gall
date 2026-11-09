import { useEffect, useRef, useState } from "react";
import { annotationKey, moveMark, normalizedBox, type AnnotationTool, type Mark } from "../core/bugReport";
import { flattenScreenshot } from "../core/reportMedia";
import { useStore } from "../state/store";
import { Icon } from "./Icon";

export function useBlobUrl(blob: Blob): string {
  const [url, setUrl] = useState("");
  useEffect(() => {
    const next = URL.createObjectURL(blob); setUrl(next);
    return () => URL.revokeObjectURL(next);
  }, [blob]);
  return url;
}

export function ReportImageEditor({ blob, save, cancel, onBusy }: { blob: Blob; save: (blob: Blob) => void; cancel: () => void; onBusy: (busy: boolean) => void }) {
  const url = useBlobUrl(blob);
  const vim = useStore((s) => s.settings?.vimKeys ?? false);
  const [tool, setTool] = useState<AnnotationTool>("arrow");
  const [marks, setMarks] = useState<Mark[]>([]);
  const [pending, setPending] = useState<Mark>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const drag = useRef<Mark | undefined>(undefined);
  const stage = useRef<SVGSVGElement>(null);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  function point(e: React.PointerEvent<SVGSVGElement>) {
    const rect = e.currentTarget.getBoundingClientRect();
    return { x: Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width)), y: Math.max(0, Math.min(1, (e.clientY - rect.top) / rect.height)) };
  }
  function draw(mark: Mark, i: number) {
    const box = normalizedBox(mark), props = { key: i, stroke: "#ff574f", strokeWidth: 0.006, fill: "none" };
    if (mark.tool === "arrow") return <g {...props}><line x1={mark.x} y1={mark.y} x2={mark.endX} y2={mark.endY} markerEnd="url(#report-arrow)" /></g>;
    return <rect {...props} x={box.x} y={box.y} width={box.width} height={box.height} fill={mark.tool === "redact" ? "#111111" : "none"} stroke={mark.tool === "redact" ? "none" : props.stroke} />;
  }
  return <div className="report-editor">
    <div className="report-section-heading"><div><h3>Show us where</h3><p>Point out the problem. Cover private details with Redact.</p></div><Icon name="image" size={24} /></div>
    <div className="report-editor-tools" role="group" aria-label="Annotation tools">
      {(["arrow", "rectangle", "redact"] as const).map((value) => <button key={value} className="btn" aria-pressed={tool === value} disabled={busy} onClick={() => setTool(value)}><Icon name={value === "arrow" ? "forward" : value === "redact" ? "lock" : "window"} />{value === "rectangle" ? "Box" : value === "redact" ? "Redact" : "Arrow"}</button>)}
      <button className="btn" disabled={busy} onClick={() => { setMarks((old) => [...old, { tool, x: 0.25, y: 0.25, endX: 0.5, endY: 0.5 }]); stage.current?.focus(); }}>Add mark</button>
      <button className="btn" disabled={!marks.length || busy} onClick={() => setMarks((old) => old.slice(0, -1))}><Icon name="undo" />Undo</button>
    </div>
    <div className="report-editor-stage">
      <img src={url} alt="Screenshot being annotated" />
      <svg ref={stage} viewBox="0 0 1 1" preserveAspectRatio="none" tabIndex={0} role="application" aria-label="Screenshot annotations. Drag to draw, or Add mark then use arrow keys to move; Shift resizes. Delete removes the last mark."
        onKeyDown={(e) => {
          if (annotationKey(e.key, vim)) {
            e.preventDefault();
            setMarks((old) => old.map((mark, i) => i === old.length - 1 ? moveMark(mark, e.key, e.shiftKey) : mark));
          } else if (e.key === "Delete" || e.key === "Backspace" || ((e.ctrlKey || e.metaKey) && e.key === "z")) { e.preventDefault(); setMarks((old) => old.slice(0, -1)); }
        }}
        onPointerDown={(e) => {
          if (busy || e.button !== 0) return;
          e.preventDefault(); e.currentTarget.focus(); e.currentTarget.setPointerCapture(e.pointerId);
          const p = point(e); drag.current = { tool, ...p, endX: p.x, endY: p.y }; setPending(drag.current);
        }}
        onPointerMove={(e) => { if (!drag.current) return; const p = point(e); drag.current = { ...drag.current, endX: p.x, endY: p.y }; setPending(drag.current); }}
        onPointerCancel={() => { drag.current = undefined; setPending(undefined); }}
        onPointerUp={() => {
          const mark = drag.current;
          if (mark && (Math.abs(mark.endX - mark.x) > 0.003 || Math.abs(mark.endY - mark.y) > 0.003)) setMarks((old) => [...old, mark]);
          drag.current = undefined; setPending(undefined);
        }}>
        <defs><marker id="report-arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="5" markerHeight="5" orient="auto-start-reverse"><path d="M0 0 L10 5 L0 10" fill="none" stroke="#ff574f" strokeWidth="2" /></marker></defs>
        {marks.map(draw)}{pending && draw(pending, marks.length)}
      </svg>
    </div>
    <p className="report-hint">Drag to mark, or Add mark and use {vim ? "h/j/k/l or " : ""}arrow keys. Shift resizes. Redactions are permanently flattened when applied.</p>
    {error && <p role="alert">{error}</p>}
    <div className="report-editor-actions"><button className="btn" disabled={busy} onClick={cancel}>Cancel editing</button><button className="btn primary" disabled={busy} onClick={() => {
      setBusy(true); setError(""); onBusy(true);
      void flattenScreenshot(blob, marks).then((next) => {
        if (alive.current) { onBusy(false); save(next); }
      }).catch((e) => { if (alive.current) { setError(String(e)); setBusy(false); onBusy(false); } });
    }}>{busy ? "Applying..." : "Apply changes"}</button></div>
  </div>;
}
