// Similarity matrix for 3+ files: a heatmap of pairwise similarity (one hue,
// light → dark, every cell labelled with its percentage), files grouped so
// near-copies sit together, plus the closest pairs and the odd one out.
import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/ipc";
import type { SimilarityMatrix as M } from "../api/types";
import { formatSize } from "../core/format";
import * as A from "../state/actions";
import { FileIcon, Icon } from "./Icon";

const pct = (r: number) => `${Math.round(r * 100)}%`;
/** Sequential ramp on the accent hue; contrast flips to light text on dark cells. */
const cellStyle = (r: number) => ({
  background: `color-mix(in oklab, var(--accent) ${Math.round(8 + r * 80)}%, var(--bg-2))`,
  color: r > 0.55 ? "#fff" : "var(--fg)",
});

export function SimilarityMatrix({ paths }: { paths: string[] }) {
  const [m, setM] = useState<M | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [grouped, setGrouped] = useState(true);
  const [cur, setCur] = useState<[number, number]>([0, 1]);
  const [hover, setHover] = useState<[number, number] | null>(null);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => { api.similarityMatrix(paths).then(setM).catch((e) => setErr(String(e))); }, [paths]);
  useEffect(() => { root.current?.focus(); }, [m]);

  const order = useMemo(() => (m ? (grouped ? m.order : m.paths.map((_, i) => i)) : []), [m, grouped]);
  const n = order.length;
  const stats = useMemo(() => {
    if (!m) return null;
    const N = m.paths.length;
    const pairs: [number, number, number][] = [];
    for (let i = 0; i < N; i++) for (let j = i + 1; j < N; j++) pairs.push([i, j, m.ratios[i][j]]);
    pairs.sort((a, b) => b[2] - a[2]);
    const avg = m.paths.map((_, i) => m.ratios[i].reduce((s, r, j) => s + (i === j ? 0 : r), 0) / Math.max(1, N - 1));
    const odd = avg.indexOf(Math.min(...avg));
    return { top: pairs.slice(0, 5), avg, odd, identical: pairs.filter((p) => p[2] >= 0.9999).length };
  }, [m]);

  const open = (i: number, j: number) => {
    if (!m || i === j) return;
    A.openOverlay({ type: "diff", a: { kind: "file", path: m.paths[i] }, b: { kind: "file", path: m.paths[j] }, back: { type: "matrix", paths } });
  };
  const onKey = (e: React.KeyboardEvent) => {
    e.stopPropagation();
    const [r, c] = cur;
    const k = e.key;
    const mv = (dr: number, dc: number) => setCur([Math.max(0, Math.min(n - 1, r + dr)), Math.max(0, Math.min(n - 1, c + dc))]);
    if (k === "Escape" || k === "q") A.openOverlay(null);
    else if (k === "ArrowUp" || k === "k") mv(-1, 0);
    else if (k === "ArrowDown" || k === "j") mv(1, 0);
    else if (k === "ArrowLeft" || k === "h") mv(0, -1);
    else if (k === "ArrowRight" || k === "l") mv(0, 1);
    else if (k === "Enter") open(order[r], order[c]);
    else if (k === "o") setGrouped((g) => !g);
    else return;
    e.preventDefault();
  };

  const hl = hover ?? cur;
  return (
    <div className="backdrop glass center" onMouseDown={() => A.openOverlay(null)}>
      <div className="simx" ref={root} tabIndex={-1} onKeyDown={onKey} onMouseDown={(e) => e.stopPropagation()}>
        <header className="sx-head">
          <Icon name="compare" size={16} />
          <b>Similarity of {paths.length} files</b>
          <div className="seg">
            <button className={grouped ? "on" : ""} title="Order so similar files sit together (o)" onClick={() => setGrouped(true)}>Grouped by similarity</button>
            <button className={!grouped ? "on" : ""} title="Selection order (o)" onClick={() => setGrouped(false)}>Selection order</button>
          </div>
          <span className="spacer" />
          <button className="icon-btn" title="Close (Esc)" onClick={() => A.openOverlay(null)}><Icon name="close" size={15} /></button>
        </header>
        {err && <div className="dv-msg err">{err}</div>}
        {!m && !err && <div className="dv-msg">Comparing {paths.length} files…</div>}
        {m && stats && (
          <div className="sx-body">
            <div className="sx-grid-wrap">
              <table className="sx-grid" role="grid" aria-label="Pairwise similarity">
                <thead>
                  <tr>
                    <th />
                    {order.map((i, c) => <th key={i} className={`sx-col ${hl[1] === c || hl[0] === c ? "hl" : ""}`} title={m.names[i]}>{c + 1}</th>)}
                  </tr>
                </thead>
                <tbody>
                  {order.map((i, r) => (
                    <tr key={i}>
                      <th className={`sx-row ${hl[0] === r || hl[1] === r ? "hl" : ""} ${i === stats.odd ? "odd" : ""}`} title={m.paths[i]}>
                        <span className="sx-num">{r + 1}</span>
                        <FileIcon entry={{ isDir: false, ext: m.names[i].split(".").pop() ?? "", name: m.names[i] }} size={14} />
                        <span className="sx-name">{m.names[i]}</span>
                        <span className="sx-size">{formatSize(m.sizes[i])}</span>
                      </th>
                      {order.map((j, c) => {
                        const v = m.ratios[i][j];
                        const diag = i === j;
                        return (
                          <td key={j} className={`sx-cell ${diag ? "diag" : ""} ${cur[0] === r && cur[1] === c ? "cur" : ""}`}
                            style={diag ? undefined : cellStyle(v)}
                            title={diag ? m.names[i] : `${m.names[i]} ↔ ${m.names[j]}: ${(v * 100).toFixed(1)}% similar${m.kinds[i] !== "text" || m.kinds[j] !== "text" ? " (byte comparison)" : ""} · click to compare`}
                            onMouseEnter={() => setHover([r, c])} onMouseLeave={() => setHover(null)}
                            onClick={() => { setCur([r, c]); open(i, j); }}>
                            {diag ? "" : v >= 0.9999 ? "=" : pct(v)}
                          </td>
                        );
                      })}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <aside className="sx-side">
              <h4>Most alike</h4>
              {stats.top.map(([i, j, r]) => (
                <button key={`${i}-${j}`} className="sx-pair" onClick={() => open(i, j)}>
                  <span className="sx-chip" style={cellStyle(r)}>{r >= 0.9999 ? "=" : pct(r)}</span>
                  <span>{m.names[i]}</span><span className="sx-arrow">↔</span><span>{m.names[j]}</span>
                </button>
              ))}
              <h4>Odd one out</h4>
              <div className="sx-odd"><b>{m.names[stats.odd]}</b> · on average {pct(stats.avg[stats.odd])} similar to the others</div>
              {stats.identical > 0 && <div className="sx-note"><Icon name="check" size={13} /> {stats.identical} identical pair{stats.identical === 1 ? "" : "s"} (=)</div>}
              <h4>Scale</h4>
              <div className="sx-legend"><span>0%</span><i /><span>100%</span></div>
              <p className="sx-hint">Text files: share of matching lines. Binary or very large files: identical or not. Click a cell (or Enter) to see the differences.</p>
            </aside>
          </div>
        )}
      </div>
    </div>
  );
}
