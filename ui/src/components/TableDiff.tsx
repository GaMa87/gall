// Table comparison (CSV/TSV, Excel, ODS): sheets as tabs, columns matched by name,
// rows by a key column or by position, changed cells shown as old → new.
import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api/ipc";
import type { TableDiffOptions, TableDiffResult } from "../api/types";
import { basename, join, parent, splitExt } from "../core/path";
import * as A from "../state/actions";
import { useStore } from "../state/store";
import { changeReport } from "../core/tableReport";
import { Icon } from "./Icon";

export const TABLE_EXT = new Set(["csv", "tsv", "tab", "xlsx", "xlsm", "xls", "xlsb", "ods"]);
const SHEET_EXT = new Set(["xlsx", "xlsm", "xls", "xlsb", "ods"]);
const ext = (p: string) => splitExt(basename(p))[1].slice(1).toLowerCase();
export const isTable = (p: string) => TABLE_EXT.has(ext(p));
const OPTS_KEY = "tablediff:opts";

function loadOpts(): TableDiffOptions {
  const d: TableDiffOptions = { key: null, header: true, numeric: true, tolerance: 1e-9, ignoreWhitespace: false, ignoreCase: false, formulas: false };
  try {
    return { ...d, ...JSON.parse(localStorage.getItem(OPTS_KEY) ?? "{}"), key: null };
  } catch {
    return d;
  }
}

export function TableDiff({ left, right, onText }: { left: string; right: string; onText?: () => void }) {
  const fontSize = useStore((s) => s.settings?.fontSize ?? 13);
  const [opts, setOpts] = useState<TableDiffOptions>(loadOpts);
  const [res, setRes] = useState<TableDiffResult | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [sheet, setSheet] = useState(0);
  const [onlyChanges, setOnlyChanges] = useState(true);
  const [cur, setCur] = useState(-1);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(500);
  const body = useRef<HTMLDivElement>(null);
  const rowH = Math.round(fontSize * 1.9);
  const spreadsheet = SHEET_EXT.has(ext(left)) || SHEET_EXT.has(ext(right));

  useEffect(() => {
    setLoading(true);
    setErr(null);
    api.tableCompare(left, right, opts)
      .then((r) => { setRes(r); setSheet((s) => Math.min(s, Math.max(0, r.sheets.length - 1))); setCur(-1); })
      .catch((e) => setErr(String(e)))
      .finally(() => setLoading(false));
  }, [left, right, opts]);
  useEffect(() => { const { key: _k, ...rest } = opts; localStorage.setItem(OPTS_KEY, JSON.stringify(rest)); }, [opts]);
  useEffect(() => {
    const el = body.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setViewH(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, [res]);

  const s = res?.sheets[sheet] ?? null;
  const rows = useMemo(() => (s ? (onlyChanges ? s.rows.filter((r) => r.status !== "same") : s.rows) : []), [s, onlyChanges]);
  const changeIdx = useMemo(() => rows.map((r, i) => (r.status !== "same" ? i : -1)).filter((i) => i >= 0), [rows]);
  const go = (d: 1 | -1) => {
    if (!changeIdx.length) return;
    const pos = changeIdx.findIndex((i) => (d > 0 ? i > cur : i >= cur));
    const k = d > 0 ? (pos < 0 ? 0 : pos) : (pos <= 0 ? changeIdx.length - 1 : pos - 1);
    const i = changeIdx[k];
    setCur(i);
    if (body.current) body.current.scrollTop = Math.max(0, i * rowH - body.current.clientHeight / 3);
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).tagName === "SELECT" || (e.target as HTMLElement).tagName === "INPUT") return;
      if (e.key === "n" || e.key === "]") go(1);
      else if (e.key === "N" || e.key === "[") go(-1);
      else if (e.key === "c") setOnlyChanges((v) => !v);
      else return;
      e.stopPropagation();
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  const exportReport = async () => {
    if (!res) return;
    const { save } = await import("@tauri-apps/plugin-dialog");
    const dir = parent(right);
    const dest = await save({ defaultPath: dir ? join(dir, "changes.csv") : "changes.csv", filters: [{ name: "CSV", extensions: ["csv"] }] });
    if (!dest) return;
    const text = res.sheets.map(changeReport).map((t, i) => (i ? t.split("\n").slice(1).join("\n") : t)).join("");
    api.saveText(dest, text).then(() => A.notify(`Saved ${basename(dest)}`, "success")).catch(A.fail);
  };

  const val = (side: "l" | "r", i: number | null, c: number | null) => (s && i != null && c != null ? (side === "l" ? s.leftRows : s.rightRows)[i]?.[c] ?? "" : "");
  const first = Math.max(0, Math.floor(scrollTop / rowH) - 15);
  const last = Math.min(rows.length, Math.ceil((scrollTop + viewH) / rowH) + 15);
  // Fixed widths (ch) from the content, shared by header and rows (rows are virtualised).
  const colTemplate = useMemo(() => {
    if (!s) return "";
    const sample = s.rows.length > 3000 ? s.rows.filter((r) => r.status !== "same").slice(0, 3000) : s.rows;
    const w = s.columns.map((c) => {
      let m = c.name.length + 3;
      for (const r of sample) {
        const lv = r.left != null && c.left != null ? s.leftRows[r.left]?.[c.left]?.length ?? 0 : 0;
        const rv = r.right != null && c.right != null ? s.rightRows[r.right]?.[c.right]?.length ?? 0 : 0;
        m = Math.max(m, r.status === "changed" && r.cells.length ? lv + rv + 2 : Math.max(lv, rv));
      }
      return `${Math.max(6, Math.min(40, m)) + 2}ch`;
    });
    return `6ch ${w.join(" ")}`;
  }, [s]);
  const addedCols = s?.columns.filter((c) => c.left == null).map((c) => c.name) ?? [];
  const removedCols = s?.columns.filter((c) => c.right == null).map((c) => c.name) ?? [];

  return (
    <div className="tblcmp">
      <div className="tb-bar">
        {res && res.sheets.length > 1 && (
          <div className="tb-sheets">
            {res.sheets.map((sh, i) => {
              const n = sh.counts.added + sh.counts.removed + sh.counts.changed;
              return (
                <button key={sh.name} className={`tb-sheet ${i === sheet ? "on" : ""} ${sh.presence}`} title={sh.presence === "both" ? `${n} changed rows` : `only on the ${sh.presence === "leftOnly" ? "left" : "right"}`} onClick={() => setSheet(i)}>
                  {sh.name}{sh.presence !== "both" ? (sh.presence === "leftOnly" ? " ◧" : " ◨") : n ? <b>{n}</b> : <span className="ok">=</span>}
                </button>
              );
            })}
          </div>
        )}
        {s && (
          <label className="tb-key" title="Rows are matched by this column. Position = row by row.">
            Match rows by
            <select value={opts.key ?? s.key ?? ""} onChange={(e) => setOpts((o) => ({ ...o, key: e.target.value }))}>
              <option value="">position</option>
              {s.columns.filter((c) => c.left != null && c.right != null).map((c) => (
                <option key={c.name} value={c.name}>{c.name}{s.keyCandidates.includes(c.name) ? " (unique)" : ""}</option>
              ))}
            </select>
          </label>
        )}
        <div className="dv-toggles">
          <button className={`tgl ${opts.header ? "on" : ""}`} title="First row is the header" onClick={() => setOpts((o) => ({ ...o, header: !o.header, key: null }))}>H</button>
          <button className={`tgl ${opts.numeric ? "on" : ""}`} title="Compare numbers by value (1.0 = 1, 2,5 = 2.5)" onClick={() => setOpts((o) => ({ ...o, numeric: !o.numeric }))}>1.0=1</button>
          <button className={`tgl ${opts.ignoreWhitespace ? "on" : ""}`} title="Ignore whitespace" onClick={() => setOpts((o) => ({ ...o, ignoreWhitespace: !o.ignoreWhitespace }))}>␣</button>
          <button className={`tgl ${opts.ignoreCase ? "on" : ""}`} title="Ignore case" onClick={() => setOpts((o) => ({ ...o, ignoreCase: !o.ignoreCase }))}>Aa</button>
          {spreadsheet && <button className={`tgl ${opts.formulas ? "on" : ""}`} title="Compare formulas instead of values" onClick={() => setOpts((o) => ({ ...o, formulas: !o.formulas }))}>fx</button>}
        </div>
        <button className={`dc-chip ${onlyChanges ? "on" : ""}`} title="Show only changed rows (c)" onClick={() => setOnlyChanges((v) => !v)}>Changes only</button>
        <span className="spacer" />
        {changeIdx.length > 0 && (
          <div className="dv-nav">
            <button className="icon-btn" title="Previous change (N)" onClick={() => go(-1)}><Icon name="up" size={15} /></button>
            <span className="dv-count">{cur >= 0 ? changeIdx.indexOf(cur) + 1 : "–"} / {changeIdx.length}</span>
            <button className="icon-btn" title="Next change (n)" onClick={() => go(1)}><Icon name="down" size={15} /></button>
          </div>
        )}
        {onText && <button className="dc-btn" title="Compare as plain text" onClick={onText}>Text view</button>}
        {res && <button className="icon-btn" title="Export the changes as CSV" onClick={exportReport}><Icon name="download" size={15} /></button>}
      </div>

      {s && (
        <div className="tb-summary">
          <span className="add">+{s.counts.added} rows</span>
          <span className="del">−{s.counts.removed} rows</span>
          <span className="chg">{s.counts.cells} cells changed in {s.counts.changed} rows</span>
          <span className="same">{s.counts.same} same</span>
          {addedCols.length > 0 && <span className="add">columns +{addedCols.join(", +")}</span>}
          {removedCols.length > 0 && <span className="del">columns −{removedCols.join(", −")}</span>}
          <span className="key">{s.key ? `matched by “${s.key}”` : "matched by position"}</span>
          {s.truncated && <span className="del">truncated at 200,000 rows</span>}
        </div>
      )}
      {err && <div className="dv-msg err">{err}</div>}
      {loading && !res && <div className="dv-msg">Reading tables…</div>}
      {s && rows.length === 0 && <div className="dv-msg same"><Icon name="check" size={16} /> No differences{onlyChanges && s.counts.same ? ` (${s.counts.same} identical rows)` : ""}</div>}

      {s && rows.length > 0 && (
        <div className="tb-body" ref={body} onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}>
          <div className="tb-hrow" style={{ gridTemplateColumns: colTemplate }}>
            <div className="tb-h num">#</div>
            {s.columns.map((c) => (
              <div key={c.name} className={`tb-h ${c.left == null ? "add" : c.right == null ? "del" : ""} ${c.name === s.key ? "key" : ""}`} title={c.left == null ? "column only on the right" : c.right == null ? "column only on the left" : c.name === s.key ? "key column" : c.name}>
                {c.name === s.key && "🔑 "}{c.name}
              </div>
            ))}
          </div>
          <div className="tb-rows" style={{ height: rows.length * rowH }}>
            {rows.slice(first, last).map((r, k) => {
              const i = first + k;
              const changed = new Set(r.cells);
              return (
                <div key={i} className={`tb-row ${r.status} ${i === cur ? "cur" : ""}`} style={{ top: i * rowH, height: rowH, gridTemplateColumns: colTemplate }} onClick={() => setCur(i)}>
                  <div className="tb-c num" title={`left row ${r.left != null ? r.left + 1 + (opts.header ? 1 : 0) : "–"} · right row ${r.right != null ? r.right + 1 + (opts.header ? 1 : 0) : "–"}`}>
                    {r.status === "added" ? "+" : r.status === "removed" ? "−" : r.status === "changed" ? "~" : ""}{(r.right ?? r.left ?? 0) + 1}
                  </div>
                  {s.columns.map((c, ci) => {
                    const lv = val("l", r.left, c.left), rv = val("r", r.right, c.right);
                    if (r.status === "changed" && changed.has(ci)) {
                      return <div key={ci} className="tb-c chg" title={`${lv || "(empty)"} → ${rv || "(empty)"}`}><s>{lv}</s><span>{rv}</span></div>;
                    }
                    const v = r.status === "removed" ? lv : rv || lv;
                    return <div key={ci} className="tb-c" title={v}>{v}</div>;
                  })}
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
