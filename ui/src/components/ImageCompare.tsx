// Image comparison inside the compare view: side by side, swipe (drag the divider),
// onion skin (opacity) and a pixel difference rendered by the backend.
import { useEffect, useRef, useState } from "react";
import { tt } from "../i18n";
import { api, fileSrc } from "../api/ipc";
import type { ImageDiff } from "../api/types";
import {
  IMAGE_EXT,
  type ImageDims,
  canPixelDiff,
  fitImagePair,
  formatDiffSummary,
  imageByteLimitMessage,
  imagePixelLimitMessage,
  isImagePath,
} from "../core/imageCompare";
import { getState } from "../state/store";

export { IMAGE_EXT };
export const isImage = isImagePath;

type Mode = "side" | "swipe" | "onion" | "diff";
const MODES: [Mode, string][] = [["side", "Side by side"], ["swipe", "Swipe"], ["onion", "Onion skin"], ["diff", "Difference"]];

/** `embedded`: only one part of a window, so its keys work while it has the focus. `stamp` shows a file that changed under its name again. */
export function ImageCompare({
  left,
  right,
  leftSize,
  rightSize,
  embedded,
  labels = [tt("left"), tt("right")],
  stamp: version,
}: {
  left: string;
  right: string;
  leftSize?: number | null;
  rightSize?: number | null;
  embedded?: boolean;
  labels?: [string, string];
  stamp?: number;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [mode, setMode] = useState<Mode>(() => (localStorage.getItem("imgcmp:mode") as Mode) || "side");
  const [split, setSplit] = useState(50);
  const [opacity, setOpacity] = useState(50);
  const [diff, setDiff] = useState<ImageDiff | null>(null);
  const [diffErr, setDiffErr] = useState<string | null>(null);
  const [dims, setDims] = useState<[ImageDims | null, ImageDims | null]>([null, null]);
  const [stackBox, setStackBox] = useState<{ width: number; height: number }>({ width: 0, height: 0 });
  const stage = useRef<HTMLDivElement>(null);
  const stackRef = useRef<HTMLDivElement>(null);
  const frameRef = useRef<HTMLDivElement>(null);
  const canDiff = canPixelDiff(left, right);
  const byteLimit = imageByteLimitMessage(leftSize) ?? imageByteLimitMessage(rightSize);
  const pixelLimit = imagePixelLimitMessage(dims[0]) ?? imagePixelLimitMessage(dims[1]);
  const pairLayout = fitImagePair(dims[0], dims[1]);
  const diffSummary = diff ? formatDiffSummary(diff) : null;
  const stamp = useRef(Date.now()); // bust the webview cache: the diff PNG path is reused

  useEffect(() => { localStorage.setItem("imgcmp:mode", mode); }, [mode]);
  // A pixel result and natural dimensions belong to the two files and the versions they were made from.
  useEffect(() => { setDiff(null); setDiffErr(null); }, [left, right, version]);
  // The dimensions are cleared while rendering, not in an effect: a cached image can fire its load event between the
  // commit and a passive effect, and the effect would then wipe the size that image reported once.
  const pairKey = `${left}\0${right}\0${version ?? ""}`;
  const [dimsKey, setDimsKey] = useState(pairKey);
  if (dimsKey !== pairKey) {
    setDimsKey(pairKey);
    setDims([null, null]);
  }
  useEffect(() => {
    const el = stackRef.current;
    if (!el) return;
    const update = () => setStackBox({ width: el.clientWidth, height: el.clientHeight });
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, [mode, byteLimit]);
  useEffect(() => {
    if (mode !== "diff" || diff || !canDiff || byteLimit || pixelLimit) return;
    // An answer that arrives after the pair changed (or the view closed) is not taken.
    let alive = true;
    setDiffErr(null);
    stamp.current = Date.now();
    api.imageDiff(left, right).then((found) => { if (alive) setDiff(found); }).catch((e) => { if (alive) setDiffErr(String(e)); });
    return () => { alive = false; };
  }, [mode, left, right, version, diff, canDiff, byteLimit, pixelLimit]);

  // Keys 1–4 switch modes; [ and ] move the divider / opacity.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (embedded && !box.current?.contains(document.activeElement)) return;
      if (getState().overlay?.type === "dialog") return; // a dialog over the compare view has the keys
      const i = ["1", "2", "3", "4"].indexOf(e.key);
      if (i >= 0) {
        if (MODES[i][0] === "diff" && !canDiff) return;
        setMode(MODES[i][0]);
      } else if (e.key === "[" || e.key === "]") {
        const d = e.key === "]" ? 5 : -5;
        if (mode === "swipe") setSplit((s) => Math.max(0, Math.min(100, s + d)));
        if (mode === "onion") setOpacity((o) => Math.max(0, Math.min(100, o + d)));
      } else return;
      e.stopPropagation();
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [mode, embedded, canDiff]);

  const onLoad = (i: 0 | 1) => (e: React.SyntheticEvent<HTMLImageElement>) => {
    const im = e.currentTarget;
    if (!im.naturalWidth || !im.naturalHeight) return;
    setDims((d) => {
      if (d[i]?.width === im.naturalWidth && d[i]?.height === im.naturalHeight) return d;
      const n: [ImageDims | null, ImageDims | null] = [d[0], d[1]];
      n[i] = { width: im.naturalWidth, height: im.naturalHeight };
      return n;
    });
  };
  const drag = (e: React.MouseEvent) => {
    if (mode !== "swipe" || (!(e.buttons & 1) && e.type === "mousemove")) return;
    const target = frameRef.current ?? stage.current;
    if (!target) return;
    const r = target.getBoundingClientRect();
    if (r.width <= 0) return;
    setSplit(Math.max(0, Math.min(100, ((e.clientX - r.left) / r.width) * 100)));
  };
  const query = version ? `?v=${version}` : "", a = fileSrc(left) + query, b = fileSrc(right) + query;
  const frameStyle = pairLayout && stackBox.width > 0 && stackBox.height > 0
    ? (() => {
        const k = Math.min(stackBox.width / pairLayout.stageWidth, stackBox.height / pairLayout.stageHeight);
        return {
          width: Math.max(1, Math.round(pairLayout.stageWidth * k)),
          height: Math.max(1, Math.round(pairLayout.stageHeight * k)),
        };
      })()
    : undefined;
  const dimText = (d: ImageDims | null) => (d ? `${d.width}×${d.height}` : "…");

  if (byteLimit) {
    return (
      <div className="imgcmp" ref={box} tabIndex={embedded ? 0 : undefined}>
        <div className="dv-msg">{tt(byteLimit)}</div>
      </div>
    );
  }

  return (
    <div className="imgcmp" ref={box} tabIndex={embedded ? 0 : undefined}>
      <div className="ic-bar">
        <div className="seg" role="tablist">
          {MODES.map(([m, label], i) => (
            <button key={m} className={mode === m ? "on" : ""} disabled={m === "diff" && !canDiff} title={`${label} (${i + 1})`} onClick={() => setMode(m)}>{label}</button>
          ))}
        </div>
        {mode === "swipe" && <input type="range" min={0} max={100} value={split} onChange={(e) => setSplit(+e.target.value)} title={tt("Divider ([ ])")} />}
        {mode === "onion" && <input type="range" min={0} max={100} value={opacity} onChange={(e) => setOpacity(+e.target.value)} title={tt("Right image opacity ([ ])")} />}
        <span className="ic-dims">
          {dimText(dims[0])} <span className="sep">vs</span> {dimText(dims[1])}
          {pairLayout?.sizeMismatch && <b className="warn"> · sizes differ</b>}
        </span>
        {mode === "diff" && diff && diffSummary && (
          <span className="ic-stat">
            <b>{diffSummary.percentText}%</b> of pixels changed ({diff.changed.toLocaleString()} px)
            {diffSummary.overlapText && <> · {diffSummary.overlapText}</>}
            {diffSummary.identical && " · visually identical"}
          </span>
        )}
      </div>
      <div className={`ic-stage ${mode}`} ref={stage} onMouseDown={drag} onMouseMove={drag}>
        {mode === "side" && (
          <>
            <figure><img className="ic-alpha" src={a} onLoad={onLoad(0)} draggable={false} alt="left" /><figcaption>{labels[0]}</figcaption></figure>
            <figure><img className="ic-alpha" src={b} onLoad={onLoad(1)} draggable={false} alt="right" /><figcaption>{labels[1]}</figcaption></figure>
          </>
        )}
        {(mode === "swipe" || mode === "onion") && (
          <div className="ic-stack" ref={stackRef}>
            <div className="ic-frame ic-alpha" ref={frameRef} style={frameStyle}>
              <img
                src={a}
                onLoad={onLoad(0)}
                draggable={false}
                alt="left"
                className={pairLayout ? "ic-aligned" : undefined}
                style={pairLayout ? { width: `${pairLayout.leftBox.widthPct}%`, height: `${pairLayout.leftBox.heightPct}%` } : undefined}
              />
              <div
                className="ic-overlay"
                style={mode === "swipe" ? { clipPath: `inset(0 0 0 ${split}%)` } : { opacity: opacity / 100 }}
              >
                <img
                  src={b}
                  onLoad={onLoad(1)}
                  draggable={false}
                  alt="right"
                  className={pairLayout ? "ic-aligned" : undefined}
                  style={pairLayout ? { width: `${pairLayout.rightBox.widthPct}%`, height: `${pairLayout.rightBox.heightPct}%` } : undefined}
                />
              </div>
              {mode === "swipe" && <div className="ic-divider" style={{ left: `${split}%` }} />}
            </div>
            {mode === "swipe" && <><span className="ic-tag l">{labels[0]}</span><span className="ic-tag r">{labels[1]}</span></>}
          </div>
        )}
        {mode === "diff" && (
          pixelLimit ? <div className="dv-msg">{tt(pixelLimit)}</div>
            : diffErr ? <div className="dv-msg err">{diffErr}</div>
              : diff ? <div className="ic-stack"><img className="ic-alpha" src={`${fileSrc(diff.diffPng)}?v=${stamp.current}`} draggable={false} alt="difference" /></div>
                : <div className="dv-msg">{tt("Comparing pixels…")}</div>
        )}
      </div>
    </div>
  );
}
