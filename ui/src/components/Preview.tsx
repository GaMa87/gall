// File previews (P-02…P-08). Used by the details pane and the preview window.
import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { api, fileSrc } from "../api/ipc";
import type { FileEntry, PreviewInfo, SyntaxHighlight, VideoProbe } from "../api/types";
import { segments } from "../core/diffRows";
import { isBrowsableArchive } from "../core/archive";
import { formatSize } from "../core/format";
import { basename, parent } from "../core/path";
import * as A from "../state/actions";
import { useStore } from "../state/store";
import { FileIcon, Icon } from "./Icon";

export function usePreviewInfo(entry: FileEntry | null): PreviewInfo | null {
  const [info, setInfo] = useState<PreviewInfo | null>(null);
  useEffect(() => {
    if (!entry) return setInfo(null);
    let alive = true;
    const t = setTimeout(() => {
      api.previewInfo(entry.path).then((i) => alive && setInfo(i)).catch(() => alive && setInfo(null));
    }, 60); // debounce while arrowing through a list
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [entry?.path, entry?.modified, entry?.size]);
  return info && info.entry?.path === entry?.path ? info : null;
}

/** Source text with syntax colours (the compare view's grammars: syntect + bat's extra
 *  languages). Shown plain at once; the colours follow from the backend a moment later,
 *  and unknown languages simply stay plain. */
function CodePreview({ name, text, truncated }: { name: string; text: string; truncated?: boolean }) {
  const lines = useMemo(() => text.split(/\r?\n/), [text]);
  const [hl, setHl] = useState<SyntaxHighlight | null>(null);
  useEffect(() => {
    let alive = true;
    const t = setTimeout(() => {
      api.diffHighlight(name, lines).then((h) => alive && setHl(h)).catch(() => {});
    }, 80); // not for every file passed while arrowing through a list
    return () => {
      alive = false;
      clearTimeout(t);
    };
  }, [name, lines]);
  return (
    <pre className="pv-text" title={hl?.language}>
      {hl
        ? lines.map((l, i) => (
            <Fragment key={i}>
              {segments(l, undefined, hl.lines[i]).map(([t, , cat], j) => (cat ? <span key={j} className={`sx${cat}`}>{t}</span> : t))}
              {i < lines.length - 1 && "\n"}
            </Fragment>
          ))
        : text}
      {truncated && <div style={{ color: "var(--fg-faint)", marginTop: 8 }}>… (truncated)</div>}
    </pre>
  );
}

export function Preview({ entry, info, big }: { entry: FileEntry; info: PreviewInfo | null; big?: boolean }) {
  const kind = info?.kind ?? null;
  if (!info) return <div className="pv-icon"><FileIcon entry={entry} size={big ? 96 : 56} /></div>;
  // Inside an archive the webview loads the extracted copy.
  const src = info.localPath ?? entry.path;
  switch (kind) {
    case "image":
      return <ImagePreview key={entry.path} src={fileSrc(src)} w={info.width} h={info.height} />;
    case "video":
    case "audio":
      return <MediaPreview key={entry.path} path={src} kind={kind} big={big} />;
    case "pdf":
      return <PdfPreview key={entry.path} path={src} />;
    case "note":
      return <div className="pv-note">{info.text}</div>;
    case "text":
      if (MD_EXTS.has(entry.ext)) return <MarkdownPreview key={entry.path} entry={src === entry.path ? entry : { ...entry, path: src }} fallback={info.text ?? ""} />;
      return <CodePreview key={entry.path} name={entry.name} text={info.text ?? ""} truncated={info.textTruncated} />;
    case "archive":
      return (
        <div className="pv-archive">
          <div style={{ color: "var(--fg-dim)", marginBottom: 4 }}>
            <span>{info.archiveTotal} entries{info.archiveTotal > info.archive.length ? ` (first ${info.archive.length} shown)` : ""}</span>
            {isBrowsableArchive(entry.name) && <span> · Enter opens it like a folder</span>}
          </div>
          {info.archive.map((a) => (
            <div key={a.name}>
              <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{a.isDir ? "📁 " : ""}{a.name}</span>
              <span style={{ color: "var(--fg-dim)" }}>{a.isDir ? "" : formatSize(a.size)}</span>
            </div>
          ))}
        </div>
      );
    case "folder":
      return <div className="pv-icon"><FileIcon entry={entry} size={big ? 120 : 72} /><span>{info.folderItems ?? 0} items</span></div>;
    default:
      return (
        <div className="pv-icon">
          <FileIcon entry={entry} size={big ? 96 : 56} />
          <span>{info.binary ? "Binary file" : "No preview"}</span>
        </div>
      );
  }
}

const MD_EXTS = new Set(["md", "markdown", "mdown", "mkd"]);
/** The rendered view reads the whole file (the preview text is truncated); above this it is cut. */
const MD_MAX_CHARS = 4 << 20;

/** Rendered Markdown with a Source toggle. Links: #anchors scroll, URLs open in the browser, relative paths open with the default app. */
function MarkdownPreview({ entry, fallback }: { entry: FileEntry; fallback: string }) {
  const box = useRef<HTMLDivElement>(null);
  const [text, setText] = useState<string | null>(null);
  const [source, setSource] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    fetch(fileSrc(entry.path))
      .then((r) => (r.ok ? r.text() : Promise.reject(r.statusText)))
      .then((t) => alive && setText(t.length > MD_MAX_CHARS ? t.slice(0, MD_MAX_CHARS) : t))
      .catch(() => alive && setText(fallback));
    return () => { alive = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entry.path, entry.modified, entry.size]);

  useEffect(() => {
    const el = box.current;
    if (source || text === null || !el) return;
    let alive = true;
    import("../core/markdown")
      .then(({ renderMarkdown }) => renderMarkdown(text, entry.path, fileSrc))
      .then((frag) => { if (alive) { el.replaceChildren(frag); setErr(null); } })
      .catch((e) => alive && setErr(String(e)));
    return () => { alive = false; };
  }, [text, source, entry.path]);

  const onClick = async (e: React.MouseEvent) => {
    const a = (e.target as HTMLElement).closest("a");
    const href = a?.getAttribute("href");
    if (!a || !href) return;
    e.preventDefault();
    if (href.startsWith("#")) {
      box.current?.querySelector(`#${CSS.escape(decodeURIComponent(href.slice(1)))}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
      return;
    }
    if (/^(https?|mailto):/i.test(href)) {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      openUrl(href).catch(() => {});
      return;
    }
    const { resolveRel, isRunnable } = await import("../core/markdown");
    const local = resolveRel(parent(entry.path) ?? entry.path, href);
    if (!local) return;
    if (isRunnable(local) && !(await A.confirm("Run this file?", `The link opens “${basename(local)}”, which runs as a program. Open it only if you trust where this document came from.`, "Open", true))) return;
    api.openPath(local).catch(() => {});
  };

  if (err) return <pre className="pv-text">{text}</pre>;
  return (
    <>
      {source
        ? <CodePreview name={entry.name} text={text ?? ""} />
        : <div className="pv-md" onClick={onClick}><article className="md-body" ref={box} /></div>}
      <div className="pv-badge">
        <button title="Show the Markdown source or the rendered page" onClick={() => setSource((s) => !s)}>{source ? "Rendered" : "Source"}</button>
      </div>
    </>
  );
}

/** Zoom toward the cursor with the wheel, pan by dragging (P-02). */
function ImagePreview({ src, w, h }: { src: string; w: number | null; h: number | null }) {
  const box = useRef<HTMLDivElement>(null);
  const [nat, setNat] = useState<{ w: number; h: number } | null>(w && h ? { w, h } : null);
  const [view, setView] = useState<{ s: number; x: number; y: number } | null>(null);
  const [fitted, setFitted] = useState(true);

  const fit = () => {
    const el = box.current;
    if (!el || !nat) return;
    const s = Math.min(el.clientWidth / nat.w, el.clientHeight / nat.h, 1);
    setView({ s, x: (el.clientWidth - nat.w * s) / 2, y: (el.clientHeight - nat.h * s) / 2 });
    setFitted(true);
  };

  /** Zooms by `f` around a point of the box (its centre by default). */
  const zoomBy = (f: number, cx?: number, cy?: number) => {
    const el = box.current;
    if (!view || !el) return;
    const x = cx ?? el.clientWidth / 2, y = cy ?? el.clientHeight / 2;
    const s = Math.max(0.02, Math.min(40, view.s * f));
    const k = s / view.s;
    setView({ s, x: x - (x - view.x) * k, y: y - (y - view.y) * k });
    setFitted(false);
  };
  /** Actual size (1:1), centred. */
  const actual = () => zoomBy(1 / (view?.s ?? 1));

  useEffect(fit, [nat]);
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(() => fitted && fit());
    ro.observe(el);
    return () => ro.disconnect();
  });

  return (
    <div
      className="pv-image"
      ref={box}
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.ctrlKey || e.altKey || e.metaKey) return;
        if (e.key === "+" || e.key === "=") zoomBy(1.25);
        else if (e.key === "-") zoomBy(1 / 1.25);
        else if (e.key === "0") fit();
        else if (e.key === "1") actual();
        else return;
        e.preventDefault();
        e.stopPropagation();
      }}
      onWheel={(e) => {
        if (!view) return;
        e.preventDefault();
        const r = box.current!.getBoundingClientRect();
        zoomBy(e.deltaY < 0 ? 1.2 : 1 / 1.2, e.clientX - r.left, e.clientY - r.top);
      }}
      onMouseDown={(e) => {
        if (e.button !== 0 || !view) return;
        e.preventDefault();
        const sx = e.clientX - view.x;
        const sy = e.clientY - view.y;
        const move = (ev: MouseEvent) => {
          setView((v) => (v ? { ...v, x: ev.clientX - sx, y: ev.clientY - sy } : v));
          setFitted(false);
        };
        const up = () => {
          window.removeEventListener("mousemove", move);
          window.removeEventListener("mouseup", up);
        };
        window.addEventListener("mousemove", move);
        window.addEventListener("mouseup", up);
      }}
      onDoubleClick={fit}
    >
      <img
        src={src}
        draggable={false}
        onLoad={(e) => {
          const i = e.currentTarget;
          if (!nat || nat.w !== i.naturalWidth) setNat({ w: i.naturalWidth, h: i.naturalHeight });
        }}
        style={view && nat ? { width: nat.w, height: nat.h, transform: `translate(${view.x}px, ${view.y}px) scale(${view.s})` } : { opacity: 0 }}
      />
      {nat && (
        <div className="pv-badge" onMouseDown={(e) => e.stopPropagation()} onDoubleClick={(e) => e.stopPropagation()}>
          <span>{nat.w} × {nat.h}</span>
          <button title="Zoom out (−, wheel)" onClick={() => zoomBy(1 / 1.25)}>−</button>
          {view && <span className="pct">{Math.round(view.s * 100)}%</span>}
          <button title="Zoom in (+, wheel)" onClick={() => zoomBy(1.25)}>+</button>
          <button title="Actual size (1)" onClick={actual}>1:1</button>
          {/* Always there (disabled when fitted), so the buttons never move under the pointer. */}
          <button title="Fit (0, double-click)" onClick={fit} disabled={fitted}>Fit</button>
        </div>
      )}
    </div>
  );
}

/** Video/audio with keyboard seeking (P-05): ←/→ seek, [ / ] frame step. */
function MediaPreview({ path, kind, big }: { path: string; kind: "video" | "audio"; big?: boolean }) {
  const settings = useStore((s) => s.settings)!;
  const ref = useRef<HTMLVideoElement & HTMLAudioElement>(null);
  const [err, setErr] = useState<string | null>(null);
  const seek = (dir: number) => {
    const m = ref.current;
    if (!m || !isFinite(m.duration)) return;
    const step = settings.videoSeek === "fraction" ? m.duration / Math.max(2, settings.videoSeekFraction) : settings.videoSeekSeconds;
    m.currentTime = Math.max(0, Math.min(m.duration, m.currentTime + dir * step));
  };
  const onKey = (e: React.KeyboardEvent) => {
    const m = ref.current;
    if (!m) return;
    if (e.key === "ArrowRight") seek(1);
    else if (e.key === "ArrowLeft") seek(-1);
    else if (e.key === "]") { m.pause(); m.currentTime += 1 / 30; }
    else if (e.key === "[") { m.pause(); m.currentTime = Math.max(0, m.currentTime - 1 / 30); }
    else if (e.key === "k" || e.key === "K") (m.paused ? m.play() : m.pause());
    else return;
    e.preventDefault();
    e.stopPropagation();
  };
  if (err) return <MediaFallback path={path} kind={kind} err={err} />;
  const common = {
    ref,
    src: fileSrc(path),
    controls: true,
    autoPlay: big && settings.autoplayMedia,
    // In the preview window the player takes focus, so ←/→, [ ] and k work right away.
    autoFocus: big,
    onKeyDown: onKey,
    tabIndex: 0,
    onError: () => {
      const me = ref.current?.error;
      const what = ["", "aborted", "network error", "decoding failed", "format or codec not supported"][me?.code ?? 0] || "unsupported format";
      setErr(me?.message ? `${what}: ${me.message}` : what);
    },
    className: "pv-media",
  };
  return kind === "video" ? <video {...common} /> : <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 12 }}><Icon name="audio" size={64} /><audio {...common} /></div>;
}

const CODEC_NAMES: Record<string, string> = {
  prores: "Apple ProRes", dnxhd: "Avid DNxHD/DNxHR", hevc: "H.265 / HEVC", h264: "H.264", cfhd: "GoPro CineForm",
  mpeg2video: "MPEG-2", mpeg4: "MPEG-4 Part 2", mjpeg: "Motion JPEG", av1: "AV1", vp9: "VP9", qtrle: "QuickTime Animation",
};

/**
 * The built-in player failed (codec not available to the webview). With FFmpeg installed:
 * a still frame, the codec and which package to install; always "Open in player".
 */
function MediaFallback({ path, kind, err }: { path: string; kind: "video" | "audio"; err: string }) {
  const [p, setP] = useState<VideoProbe | null>(null);
  const [probing, setProbing] = useState(kind === "video");
  useEffect(() => {
    if (kind !== "video") return;
    let alive = true;
    api.videoProbe(path).then((r) => alive && setP(r)).catch(() => {}).finally(() => alive && setProbing(false));
    return () => { alive = false; };
  }, [path, kind]);

  const codec = p?.codec ? `${CODEC_NAMES[p.codec] ?? p.codecLong ?? p.codec}${p.profile ? ` ${p.profile}` : ""}` : null;
  const platform = p?.platform ?? (navigator.userAgent.includes("Windows") ? "windows" : navigator.userAgent.includes("Mac") ? "macos" : "linux");
  const meta = [p?.width && p.height ? `${p.width} × ${p.height}` : "", p?.duration ? formatDuration(p.duration) : ""].filter(Boolean).join(" · ");
  return (
    <div className="pv-fallback">
      {p?.poster ? <img className="pv-poster" src={fileSrc(p.poster)} draggable={false} /> : <Icon name={kind} size={56} />}
      <div className="pv-fb-card">
        <b>{codec ? `Can't play ${codec} here` : `Can't play this ${kind}`}</b>
        {meta && <span className="pv-fb-meta">{meta}</span>}
        <small>
          {platform === "linux"
            ? <>Playback uses the system's GStreamer codecs. Install <code>gstreamer1.0-libav</code> (Debian/Ubuntu), <code>gstreamer1-libav</code> (Fedora) or <code>gst-libav</code> (Arch), then restart Gall.</>
            : platform === "windows"
              ? <>The built-in player (WebView2) only plays H.264, VP8/VP9 and AV1 video.</>
              : <>The built-in player cannot decode this format.</>}
          {!probing && kind === "video" && !p?.ffmpeg && <> Install FFmpeg to see a still frame and the codec here.</>}
        </small>
        <div className="pv-fb-actions">
          <button className="btn primary" onClick={() => api.openPath(path).catch(() => {})}>Open in player</button>
        </div>
        <small className="pv-fb-err" title="Error from the built-in player">{err}</small>
      </div>
    </div>
  );
}

function formatDuration(s: number): string {
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = Math.floor(s % 60);
  return h ? `${h}:${String(m).padStart(2, "0")}:${String(sec).padStart(2, "0")}` : `${m}:${String(sec).padStart(2, "0")}`;
}

/** PDF via pdf.js: ←/→ change page; fit width / page toggle (P-04). */
function PdfPreview({ path }: { path: string }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const box = useRef<HTMLDivElement>(null);
  /** Invisible text over the page, so it can be selected and copied. */
  const textBox = useRef<HTMLDivElement>(null);
  const [doc, setDoc] = useState<any>(null);
  const [page, setPage] = useState(1);
  const [fitWidth, setFitWidth] = useState(false);
  /** Zoom on top of the fit (1 = the page or its width fits). */
  const [zoom, setZoom] = useState(1);
  const zoomBy = (f: number) => setZoom((z) => Math.max(0.25, Math.min(8, z * f)));
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    let loaded: any = null;
    (async () => {
      try {
        const pdfjs = await import("pdfjs-dist");
        const worker = (await import("pdfjs-dist/build/pdf.worker.min.mjs?url")).default;
        pdfjs.GlobalWorkerOptions.workerSrc = worker;
        const data = await fetch(fileSrc(path)).then((r) => r.arrayBuffer());
        loaded = await pdfjs.getDocument({ data }).promise;
        if (alive) setDoc(loaded);
      } catch (e) {
        if (alive) setErr(String(e));
      }
    })();
    return () => {
      alive = false;
      loaded?.destroy?.();
    };
  }, [path]);

  useEffect(() => {
    if (!doc || !canvas.current || !box.current) return;
    let task: any = null;
    let text: any = null;
    (async () => {
      const p = await doc.getPage(page);
      const base = p.getViewport({ scale: 1 });
      const bw = box.current!.clientWidth - 16;
      const bh = box.current!.clientHeight - 16;
      const scale = (fitWidth ? bw / base.width : Math.min(bw / base.width, bh / base.height)) * zoom;
      const dpr = window.devicePixelRatio || 1;
      const vp = p.getViewport({ scale: scale * dpr });
      const c = canvas.current!;
      c.width = vp.width;
      c.height = vp.height;
      c.style.width = `${vp.width / dpr}px`;
      c.style.height = `${vp.height / dpr}px`;
      task = p.render({ canvasContext: c.getContext("2d")!, viewport: vp, canvas: c });
      const tb = textBox.current;
      if (tb) {
        tb.replaceChildren();
        tb.style.setProperty("--total-scale-factor", String(scale));
        const { TextLayer } = await import("pdfjs-dist");
        text = new TextLayer({ textContentSource: p.streamTextContent(), container: tb, viewport: p.getViewport({ scale }) });
        text.render().catch(() => {});
      }
      await task.promise.catch(() => {});
    })();
    return () => { task?.cancel?.(); text?.cancel?.(); };
  }, [doc, page, fitWidth, zoom]);

  if (err) return <div className="pv-icon"><Icon name="pdf" size={56} /><span>PDF preview failed</span></div>;
  return (
    // The page scrolls inside; the controls stay in the corner.
    <div className="pv-pdf-wrap">
      <div className="pv-pdf" ref={box} tabIndex={0} data-page={page} data-pages={doc?.numPages ?? 1}
        onKeyDown={(e) => {
          if (!doc || e.altKey || e.metaKey) return;
          if (e.key === "ArrowRight") setPage((p) => Math.min(doc.numPages, p + 1));
          else if (e.key === "ArrowLeft") setPage((p) => Math.max(1, p - 1));
          else if (e.key === "+" || e.key === "=") zoomBy(1.25);
          else if (e.key === "-") zoomBy(1 / 1.25);
          else if (e.key === "0") setZoom(1);
          else return;
          e.preventDefault();
          e.stopPropagation();
        }}
        onWheel={(e) => {
          // Ctrl+wheel zooms (the plain wheel scrolls the page).
          if (!e.ctrlKey) return;
          e.preventDefault();
          zoomBy(e.deltaY < 0 ? 1.15 : 1 / 1.15);
        }}>
        <div className="pv-pdf-page">
          <canvas ref={canvas} />
          <div className="textLayer" ref={textBox} />
        </div>
      </div>
      {doc && (
        <div className="pv-badge">
          <button title="Previous page (←)" onClick={() => setPage((p) => Math.max(1, p - 1))}>‹</button>
          <span>{page} / {doc.numPages}</span>
          <button title="Next page (→)" onClick={() => setPage((p) => Math.min(doc.numPages, p + 1))}>›</button>
          <button title="Zoom out (−, Ctrl+wheel)" onClick={() => zoomBy(1 / 1.25)}>−</button>
          <button className="pct" title="Back to fit (0)" onClick={() => setZoom(1)}>{Math.round(zoom * 100)}%</button>
          <button title="Zoom in (+, Ctrl+wheel)" onClick={() => zoomBy(1.25)}>+</button>
          <button onClick={() => { setFitWidth((f) => !f); setZoom(1); }}>{fitWidth ? "Fit page" : "Fit width"}</button>
        </div>
      )}
    </div>
  );
}
