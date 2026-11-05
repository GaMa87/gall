// Image comparison inside the compare view: side by side, swipe (drag the divider),
// onion skin (opacity) and a pixel difference rendered by the backend.
import { useEffect, useRef, useState } from "react";
import { api, fileSrc } from "../api/ipc";
import type { ImageDiff } from "../api/types";
import { getState } from "../state/store";

export const IMAGE_EXT = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "avif", "svg", "ico"]);
const PIXEL_EXT = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp"]);
const ext = (p: string) => p.split(".").pop()?.toLowerCase() ?? "";
export const isImage = (p: string) => IMAGE_EXT.has(ext(p));

type Mode = "side" | "swipe" | "onion" | "diff";
const MODES: [Mode, string][] = [["side", "Side by side"], ["swipe", "Swipe"], ["onion", "Onion skin"], ["diff", "Difference"]];

export function ImageCompare({ left, right }: { left: string; right: string }) {
  const [mode, setMode] = useState<Mode>(() => (localStorage.getItem("imgcmp:mode") as Mode) || "side");
  const [split, setSplit] = useState(50);
  const [opacity, setOpacity] = useState(50);
  const [diff, setDiff] = useState<ImageDiff | null>(null);
  const [diffErr, setDiffErr] = useState<string | null>(null);
  const [dims, setDims] = useState<[string, string]>(["", ""]);
  const stage = useRef<HTMLDivElement>(null);
  const canDiff = PIXEL_EXT.has(ext(left)) && PIXEL_EXT.has(ext(right));
  const stamp = useRef(Date.now()); // bust the webview cache: the diff PNG path is reused

  useEffect(() => { localStorage.setItem("imgcmp:mode", mode); }, [mode]);
  useEffect(() => {
    if (mode !== "diff" || diff || !canDiff) return;
    setDiffErr(null);
    stamp.current = Date.now();
    api.imageDiff(left, right).then(setDiff).catch((e) => setDiffErr(String(e)));
  }, [mode, left, right, diff, canDiff]);

  // Keys 1–4 switch modes; [ and ] move the divider / opacity.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (getState().overlay?.type === "dialog") return; // a dialog over the compare view has the keys
      const i = ["1", "2", "3", "4"].indexOf(e.key);
      if (i >= 0) setMode(MODES[i][0]);
      else if (e.key === "[" || e.key === "]") {
        const d = e.key === "]" ? 5 : -5;
        if (mode === "swipe") setSplit((s) => Math.max(0, Math.min(100, s + d)));
        if (mode === "onion") setOpacity((o) => Math.max(0, Math.min(100, o + d)));
      } else return;
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [mode]);

  const onLoad = (i: 0 | 1) => (e: React.SyntheticEvent<HTMLImageElement>) => {
    const im = e.currentTarget;
    setDims((d) => { const n: [string, string] = [...d] as [string, string]; n[i] = `${im.naturalWidth}×${im.naturalHeight}`; return n; });
  };
  const sizeMismatch = dims[0] && dims[1] && dims[0] !== dims[1];
  const drag = (e: React.MouseEvent) => {
    if (mode !== "swipe" || !(e.buttons & 1) && e.type === "mousemove") return;
    const r = stage.current!.getBoundingClientRect();
    setSplit(Math.max(0, Math.min(100, ((e.clientX - r.left) / r.width) * 100)));
  };
  const a = fileSrc(left), b = fileSrc(right);

  return (
    <div className="imgcmp">
      <div className="ic-bar">
        <div className="seg" role="tablist">
          {MODES.map(([m, label], i) => (
            <button key={m} className={mode === m ? "on" : ""} disabled={m === "diff" && !canDiff} title={`${label} (${i + 1})`} onClick={() => setMode(m)}>{label}</button>
          ))}
        </div>
        {mode === "swipe" && <input type="range" min={0} max={100} value={split} onChange={(e) => setSplit(+e.target.value)} title="Divider ([ ])" />}
        {mode === "onion" && <input type="range" min={0} max={100} value={opacity} onChange={(e) => setOpacity(+e.target.value)} title="Right image opacity ([ ])" />}
        <span className="ic-dims">{dims[0] || "…"} <span className="sep">vs</span> {dims[1] || "…"}{sizeMismatch && <b className="warn"> · sizes differ</b>}</span>
        {mode === "diff" && diff && (
          <span className="ic-stat">
            <b>{((diff.changed / Math.max(1, diff.compared)) * 100).toFixed(diff.changed && diff.changed / diff.compared < 0.001 ? 3 : 1)}%</b> of pixels changed ({diff.changed.toLocaleString()} px)
            {diff.changed === 0 && " · visually identical"}
          </span>
        )}
      </div>
      <div className={`ic-stage ${mode}`} ref={stage} onMouseDown={drag} onMouseMove={drag}>
        {mode === "side" && (
          <>
            <figure><img src={a} onLoad={onLoad(0)} draggable={false} alt="left" /><figcaption>left</figcaption></figure>
            <figure><img src={b} onLoad={onLoad(1)} draggable={false} alt="right" /><figcaption>right</figcaption></figure>
          </>
        )}
        {(mode === "swipe" || mode === "onion") && (
          <div className="ic-stack">
            <img src={a} onLoad={onLoad(0)} draggable={false} alt="left" />
            <img src={b} onLoad={onLoad(1)} draggable={false} alt="right" className="top"
              style={mode === "swipe" ? { clipPath: `inset(0 0 0 ${split}%)` } : { opacity: opacity / 100 }} />
            {mode === "swipe" && <div className="ic-divider" style={{ left: `${split}%` }} />}
            {mode === "swipe" && <><span className="ic-tag l">left</span><span className="ic-tag r">right</span></>}
          </div>
        )}
        {mode === "diff" && (
          diffErr ? <div className="dv-msg err">{diffErr}</div>
            : diff ? <div className="ic-stack"><img src={`${fileSrc(diff.diffPng)}?v=${stamp.current}`} draggable={false} alt="difference" /></div>
              : <div className="dv-msg">Comparing pixels…</div>
        )}
      </div>
    </div>
  );
}
