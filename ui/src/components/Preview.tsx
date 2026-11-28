// File previews (P-02…P-08). Used by the details pane and the preview window.
import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { tt } from "../i18n";
import { api, fileSrc, getMediaBase, mediaSrc } from "../api/ipc";
import type { FileEntry, PreviewInfo, SyntaxHighlight, VideoProbe } from "../api/types";
import {
  FFMPEG_AUDIO_CHANNELS,
  FFMPEG_AUDIO_RATE,
  MjpegFrameParser,
  PcmStereoChunker,
  choosePlaybackFps,
  ffmpegAudioUrl,
  ffmpegFrameUrl,
  ffmpegVideoUrl,
} from "../core/ffmpegStream";
import {
  decideMediaViewMode,
  detectPlatform,
  formatCodecLabel,
  formatMediaDuration,
  formatMediaMeta,
} from "../core/mediaPreview";
import { imageRefresh, loadImageSnapshot } from "../core/imageRefresh";
import { onFolderChanged } from "../state/controllers";
import { segments } from "../core/diffRows";
import { isBrowsableArchive } from "../core/archive";
import { parseStl, type StlMesh } from "../core/stl";
import { createStlViewer, type StlViewer } from "../core/stlView";
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
      api.previewInfo(entry.path).then((i) => alive && setInfo(i)).catch(() => alive && setInfo((previous) => previous?.kind === "image" ? previous : null)); // retain images through write/rename gaps
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
      return <ImagePreview key={entry.path} path={src} w={info.width} h={info.height} />;
    case "video":
    case "audio":
      return <MediaPreview key={entry.path} path={src} size={entry.size} kind={kind} big={big} preferFfmpeg={info.useFfmpeg} />;
    case "pdf":
      return <PdfPreview key={entry.path} path={src} />;
    case "model":
      return <ModelPreview key={entry.path} path={src} />;
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

/** Models above this are not loaded just to be previewed. */
const MODEL_MAX_BYTES = 256 << 20;

/** STL model: drag to rotate, wheel to zoom, double-click to reset. */
function ModelPreview({ path }: { path: string }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [mesh, setMesh] = useState<StlMesh | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    const ctl = new AbortController();
    fetch(fileSrc(path), { signal: ctl.signal })
      .then((r) => {
        if (!r.ok) throw new Error(r.statusText || "cannot read the file");
        if (Number(r.headers.get("content-length") ?? 0) > MODEL_MAX_BYTES) throw new Error("The model is too large to preview");
        return r.arrayBuffer();
      })
      .then((b) => {
        if (b.byteLength > MODEL_MAX_BYTES) throw new Error("The model is too large to preview");
        setMesh(parseStl(b));
      })
      .catch((e) => !ctl.signal.aborted && setErr(e instanceof Error ? e.message : String(e)));
    return () => ctl.abort();
  }, [path]);

  useEffect(() => {
    const el = canvas.current;
    if (!mesh || !el) return;
    let viewer: StlViewer;
    try {
      viewer = createStlViewer(el, mesh);
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
      return;
    }
    const ro = new ResizeObserver(() => viewer.resize());
    ro.observe(el);
    let drag: { x: number; y: number } | null = null;
    const down = (e: PointerEvent) => { drag = { x: e.clientX, y: e.clientY }; el.setPointerCapture(e.pointerId); };
    const move = (e: PointerEvent) => {
      if (!drag) return;
      viewer.rotate(e.clientX - drag.x, e.clientY - drag.y);
      drag = { x: e.clientX, y: e.clientY };
    };
    const up = () => { drag = null; };
    const wheel = (e: WheelEvent) => { e.preventDefault(); viewer.zoom(e.deltaY); };
    const dbl = () => viewer.reset();
    el.addEventListener("pointerdown", down);
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
    el.addEventListener("wheel", wheel, { passive: false });
    el.addEventListener("dblclick", dbl);
    return () => {
      ro.disconnect();
      el.removeEventListener("pointerdown", down);
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
      el.removeEventListener("wheel", wheel);
      el.removeEventListener("dblclick", dbl);
      viewer.dispose();
    };
  }, [mesh]);

  if (err) return <div className="pv-pdf-error">Can't show this model: {err}</div>;
  if (!mesh) return <div className="pv-icon"><Icon name="model" size={48} /><span>{tt("Loading model…")}</span></div>;
  const size = [0, 1, 2].map((i) => +(mesh.max[i] - mesh.min[i]).toPrecision(4)).join(" × ");
  return (
    <div className="pv-model">
      <canvas ref={canvas} title={tt("Drag to rotate · wheel to zoom · double-click to reset")} />
      <span className="pv-model-info">{mesh.triangles.toLocaleString()} triangles · {size}</span>
    </div>
  );
}

const MD_EXTS = new Set(["md", "markdown", "mdown", "mkd"]);
/** The rendered view reads the whole file (the preview text is truncated); above this it is cut. */
const MD_MAX_CHARS = 4 << 20;

/** Rendered Markdown with a Source toggle. Links: #anchors scroll, URLs open in the browser, relative paths open with the default app after confirmation. */
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

  const closestLink = (target: EventTarget | null) =>
    (target instanceof Element ? target : null)?.closest("a, area, [href], [xlink\\:href]") ?? null;

  const onAuxClick = (e: React.MouseEvent) => {
    if (closestLink(e.target)) e.preventDefault();
  };

  const onClick = async (e: React.MouseEvent) => {
    const link = closestLink(e.target);
    if (!link) return;
    // Always prevent default WebView navigation, even for empty href="" links.
    e.preventDefault();
    const href = link.getAttribute("href") ?? link.getAttribute("xlink:href");
    if (!href) return;
    const { activateMarkdownLink } = await import("../core/markdown");
    await activateMarkdownLink(parent(entry.path) ?? entry.path, href, {
      scrollToAnchor: (id) => {
        if (!id) return;
        box.current?.querySelector(`#${CSS.escape(id)}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
      },
      openExternal: async (url) => {
        const { openUrl } = await import("@tauri-apps/plugin-opener");
        openUrl(url).catch(() => {});
      },
      confirmLocal: (local, runnable) => (runnable
        ? A.confirm(
          tt("Run this file?"),
          tt("The link opens “{name}”, which runs as a program. Open it only if you trust where this document came from.", { name: basename(local) }),
          "Open",
          true,
        )
        : A.confirm(
          tt("Open local file?"),
          tt("The link opens “{name}” with its default application:\n\n{path}\n\nLocal files can still run programs or scripts. Open it only if you trust where this document came from.", { name: basename(local), path: local }),
          "Open",
          false,
        )),
      openLocal: (local) => {
        api.openPath(local).catch(() => {});
      },
    });
  };

  if (err) return <pre className="pv-text">{text}</pre>;
  return (
    <>
      {source
        ? <CodePreview name={entry.name} text={text ?? ""} />
        : <div className="pv-md" onClick={onClick} onAuxClick={onAuxClick}><article className="md-body" ref={box} /></div>}
      <div className="pv-badge">
        <button title={tt("Show the Markdown source or the rendered page")} onClick={() => setSource((s) => !s)}>{source ? "Rendered" : "Source"}</button>
      </div>
    </>
  );
}

/** Zoom toward the cursor with the wheel, pan by dragging (P-02). */
function ImagePreview({ path, w, h }: { path: string; w: number | null; h: number | null }) {
  const [src, setSrc] = useState<string | undefined>();
  useEffect(() => {
    const snapshots = imageRefresh(
      (signal) => loadImageSnapshot(fileSrc(path), signal),
      setSrc,
      URL.revokeObjectURL,
    );
    const dir = parent(path);
    const off = dir ? onFolderChanged(dir, snapshots.refresh) : () => {};
    // Subscribe first; read again after watch registration to close its startup gap.
    if (dir) api.watch(dir).then(snapshots.refresh).catch(() => {});
    snapshots.refresh();
    return () => {
      off();
      snapshots.stop();
      if (dir) api.unwatch(dir).catch(() => {});
    };
  }, [path]);
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
          if (!nat || nat.w !== i.naturalWidth || nat.h !== i.naturalHeight) setNat({ w: i.naturalWidth, h: i.naturalHeight });
        }}
        style={view && nat ? { width: nat.w, height: nat.h, transform: `translate(${view.x}px, ${view.y}px) scale(${view.s})` } : { opacity: 0 }}
      />
      {nat && (
        <div className="pv-badge" onMouseDown={(e) => e.stopPropagation()} onDoubleClick={(e) => e.stopPropagation()}>
          <span>{nat.w} × {nat.h}</span>
          <button title={tt("Zoom out (−, wheel)")} onClick={() => zoomBy(1 / 1.25)}>−</button>
          {view && <span className="pct">{Math.round(view.s * 100)}%</span>}
          <button title={tt("Zoom in (+, wheel)")} onClick={() => zoomBy(1.25)}>+</button>
          <button title={tt("Actual size (1)")} onClick={actual}>1:1</button>
          {/* Always there (disabled when fitted), so the buttons never move under the pointer. */}
          <button title={tt("Fit (0, double-click)")} onClick={fit} disabled={fitted}>{tt("Fit")}</button>
        </div>
      )}
    </div>
  );
}

/**
 * Native `<video>` / `<audio>` element, mounted only after explicit play (or preview-window
 * autoplay) and torn down eagerly on unmount so WebKitGTK's GStreamer `downloadbuffer`
 * stops spooling immediately when the user moves away.
 */
function NativeMediaPlayer({
  path,
  kind,
  big,
  autoPlay,
  initialSeekSec,
  onError,
}: {
  path: string;
  kind: "video" | "audio";
  big?: boolean;
  autoPlay: boolean;
  initialSeekSec: number;
  onError: (err: string) => void;
}) {
  const settings = useStore((s) => s.settings)!;
  const ref = useRef<HTMLVideoElement & HTMLAudioElement>(null);
  const loadedRef = useRef(false);
  const onErrorRef = useRef(onError);
  onErrorRef.current = onError;
  const initialSeekDoneRef = useRef(false);

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

  // Focus the player in the preview window (or after activation) so k, arrows and [ ] work at once.
  useEffect(() => {
    if (big || autoPlay) ref.current?.focus();
  }, [big, autoPlay, path]);

  // Eagerly release the media pipeline on unmount/path change: without pause + removeAttribute +
  // load, WebKitGTK can keep a GStreamer downloadbuffer writing to /var/tmp until GC runs.
  useEffect(() => {
    const el = ref.current;
    return () => {
      if (!el) return;
      try {
        el.pause();
        el.removeAttribute("src");
        el.load();
      } catch {
        // Ignore teardown errors on detached elements.
      }
    };
  }, [path]);

  // WebKitGTK can leave <video>/<audio> stuck at readyState 0 without firing onError when a
  // GStreamer demuxer/decoder plugin is missing. Detect the stall and hand off to fallback.
  useEffect(() => {
    loadedRef.current = false;
    initialSeekDoneRef.current = false;
    const check = (hard: boolean) => {
      if (loadedRef.current) return;
      const m = ref.current;
      if (!m) return;
      if (m.error) {
        const what = ["", "aborted", "network error", "decoding failed", "format or codec not supported"][m.error.code] || "unsupported format";
        onErrorRef.current(m.error.message ? `${what}: ${m.error.message}` : what);
        return;
      }
      if (m.readyState > 0) {
        loadedRef.current = true;
        return;
      }
      if (hard || m.networkState !== HTMLMediaElement.NETWORK_LOADING) {
        onErrorRef.current("format or codec not supported");
      }
    };
    const fast = setTimeout(() => check(false), 700);
    const slow = setTimeout(() => check(true), 3000);
    return () => { clearTimeout(fast); clearTimeout(slow); };
  }, [path]);

  const markLoaded = () => {
    loadedRef.current = true;
    const m = ref.current;
    if (m && initialSeekSec > 0 && !initialSeekDoneRef.current && isFinite(m.duration)) {
      initialSeekDoneRef.current = true;
      m.currentTime = Math.min(m.duration, initialSeekSec);
    }
  };
  const common = {
    ref,
    src: mediaSrc(path),
    controls: true,
    preload: (autoPlay ? "auto" : "metadata") as "auto" | "metadata",
    autoPlay,
    onKeyDown: onKey,
    onLoadedMetadata: markLoaded,
    onLoadedData: markLoaded,
    onCanPlay: markLoaded,
    tabIndex: 0,
    onError: () => {
      const me = ref.current?.error;
      const what = ["", "aborted", "network error", "decoding failed", "format or codec not supported"][me?.code ?? 0] || "unsupported format";
      onErrorRef.current(me?.message ? `${what}: ${me.message}` : what);
    },
    className: "pv-media",
  };
  return kind === "video" ? <video {...common} /> : <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 12 }}><Icon name="audio" size={64} /><audio {...common} /></div>;
}

/**
 * Zero-spool video/audio preview (P-05):
 * - On hover/selection (and in the preview modal when autoplayMedia is off), shows a cached
 *   poster frame + metadata + Play button without mounting `<video>`/`<audio>`, so WebKitGTK
 *   never copies gigabytes into `/var/tmp/WebKit-Media-*`.
 * - On explicit Play, routes large Linux media (or missing GStreamer plugins) through the
 *   zero-spool FFmpeg streaming player, or guards multi-hundred-MB files when FFmpeg is absent.
 */
function MediaPreview({
  path,
  size,
  kind,
  big,
  preferFfmpeg,
}: {
  path: string;
  size: number;
  kind: "video" | "audio";
  big?: boolean;
  preferFfmpeg?: boolean;
}) {
  const settings = useStore((s) => s.settings)!;
  const standbyRef = useRef<HTMLDivElement>(null);
  const [probe, setProbe] = useState<VideoProbe | null>(null);
  const [probing, setProbing] = useState(true);
  const [activated, setActivated] = useState(false);
  const [autoPlayOnStart, setAutoPlayOnStart] = useState(false);
  const [initialSeekSec, setInitialSeekSec] = useState(0);
  const [allowLargeNative, setAllowLargeNative] = useState(false);
  const [nativeErr, setNativeErr] = useState<string | null>(null);
  const [ffFailed, setFfFailed] = useState(false);

  useEffect(() => {
    let alive = true;
    setProbe(null);
    setProbing(true);
    setActivated(false);
    setAutoPlayOnStart(false);
    setInitialSeekSec(0);
    setAllowLargeNative(false);
    setNativeErr(null);
    setFfFailed(false);
    api.videoProbe(path)
      .then((r) => alive && setProbe(r))
      .catch(() => {})
      .finally(() => alive && setProbing(false));
    return () => { alive = false; };
  }, [path, kind]);

  const mediaBase = getMediaBase();
  const platform = detectPlatform(probe);
  const mode = decideMediaViewMode({
    activated,
    big: Boolean(big),
    autoplayMedia: settings.autoplayMedia,
    preferFfmpeg: Boolean(preferFfmpeg),
    probe,
    probing,
    mediaBase,
    sizeBytes: size,
    isLinux: platform === "linux",
    allowLargeNative,
    nativeErr,
    ffFailed,
  });

  useEffect(() => {
    if (big && mode === "standby") standbyRef.current?.focus();
  }, [big, mode, path]);

  const activate = (playNow: boolean, seekSec = 0) => {
    setAutoPlayOnStart(playNow);
    setInitialSeekSec(Math.max(0, seekSec));
    setActivated(true);
  };

  const onStandbyKey = (e: React.KeyboardEvent) => {
    const duration = probe?.duration && Number.isFinite(probe.duration) && probe.duration > 0 ? probe.duration : 0;
    const step = settings.videoSeek === "fraction" && duration > 0
      ? duration / Math.max(2, settings.videoSeekFraction)
      : settings.videoSeekSeconds;
    const fps = choosePlaybackFps(probe?.fps);
    if (e.key === "k" || e.key === "K" || e.key === "Enter") activate(true, 0);
    else if (e.key === "ArrowRight") activate(true, step);
    else if (e.key === "ArrowLeft") activate(false, 0);
    else if (e.key === "]" && kind === "video") activate(false, 1 / fps);
    else if (e.key === "[" && kind === "video") activate(false, 0);
    else return;
    e.preventDefault();
    e.stopPropagation();
  };

  if (mode === "ffmpeg" && probe && mediaBase) {
    return (
      <FfmpegMediaPlayer
        path={path}
        kind={kind}
        probe={probe}
        big={big}
        mediaBase={mediaBase}
        initialPlaying={autoPlayOnStart || Boolean(big && settings.autoplayMedia)}
        initialSeekSec={initialSeekSec}
        onFatal={() => setFfFailed(true)}
      />
    );
  }

  if (mode === "native") {
    return (
      <NativeMediaPlayer
        path={path}
        kind={kind}
        big={big}
        autoPlay={autoPlayOnStart || Boolean(big && settings.autoplayMedia)}
        initialSeekSec={initialSeekSec}
        onError={(msg) => setNativeErr(msg)}
      />
    );
  }

  if (mode === "large-guardrail") {
    const meta = formatMediaMeta(probe, size);
    return (
      <div className="pv-fallback pv-guardrail">
        {probe?.poster ? <img className="pv-poster" src={fileSrc(probe.poster)} draggable={false} /> : <Icon name={kind} size={56} />}
        <div className="pv-fb-card">
          <b>{kind === "video" ? tt("Large video ({size})", { size: formatSize(size) }) : tt("Large audio ({size})", { size: formatSize(size) })}</b>
          {meta && <span className="pv-fb-meta">{meta}</span>}
          <small>
            {tt("Built-in Linux playback buffers HTTP streams to /var/tmp. Open it in an external player for zero-copy playback, or install")} <code>ffmpeg</code> {tt("for zero-copy in-app playback.")}
          </small>
          <div className="pv-fb-actions">
            <button type="button" className="btn primary" onClick={() => api.openPath(path).catch(() => {})}>{tt("Open in player")}</button>
            <button type="button" className="btn" onClick={() => setAllowLargeNative(true)}>{tt("Play here anyway")}</button>
          </div>
        </div>
      </div>
    );
  }

  if (mode === "fallback") {
    return (
      <MediaFallbackCard
        path={path}
        kind={kind}
        probe={probe}
        probing={probing}
        err={nativeErr ?? "format or codec not supported"}
      />
    );
  }

  // Standby zero-spool poster/card mode.
  const meta = formatMediaMeta(probe, size);
  const waitingForPlay = activated && probing;
  return (
    <div className="pv-standby" ref={standbyRef} tabIndex={0} onKeyDown={onStandbyKey}>
      {kind === "video" ? (
        <div className="pv-poster-stage" onClick={() => activate(true)}>
          {probe?.poster ? (
            <img className="pv-poster-img" src={fileSrc(probe.poster)} draggable={false} />
          ) : (
            <div className="pv-standby-placeholder">
              <Icon name="video" size={big ? 72 : 52} />
            </div>
          )}
          <button
            type="button"
            className="pv-play-overlay"
            title={tt("Play (k)")}
            aria-label={tt("Play (k)")}
            onClick={(e) => { e.stopPropagation(); activate(true); }}
          >
            {waitingForPlay ? "…" : "▶"}
          </button>
        </div>
      ) : (
        <div className="pv-audio-standby" onClick={() => activate(true)}>
          <Icon name="audio" size={big ? 64 : 48} />
          <button
            type="button"
            className="btn primary pv-audio-play-btn"
            title={tt("Play (k)")}
            onClick={(e) => { e.stopPropagation(); activate(true); }}
          >
            {waitingForPlay ? tt("Loading preview…") : `▶ ${tt("Play")}`}
          </button>
        </div>
      )}
      <div className="pv-meta-bar" onClick={(e) => e.stopPropagation()}>
        <span className="pv-fb-meta">{meta || (probing ? tt("Loading preview…") : "")}</span>
        <button
          type="button"
          className="pv-open-ext"
          title={tt("Open in player")}
          onClick={() => api.openPath(path).catch(() => {})}
        >
          {tt("Open in player")}
        </button>
      </div>
    </div>
  );
}

async function drawJpegOnCanvas(canvas: HTMLCanvasElement, bytes: Uint8Array): Promise<void> {
  const ctx = canvas.getContext("2d");
  if (!ctx) return;
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  const blob = new Blob([copy], { type: "image/jpeg" });
  if (typeof createImageBitmap === "function") {
    try {
      const bmp = await createImageBitmap(blob);
      try {
        if (canvas.width !== bmp.width || canvas.height !== bmp.height) {
          canvas.width = bmp.width;
          canvas.height = bmp.height;
        }
        ctx.drawImage(bmp, 0, 0);
      } finally {
        bmp.close();
      }
      return;
    } catch {
      // Fall through to HTMLImageElement decoding.
    }
  }
  const url = URL.createObjectURL(blob);
  try {
    await new Promise<void>((resolve, reject) => {
      const img = new Image();
      img.onload = () => {
        if (canvas.width !== img.naturalWidth || canvas.height !== img.naturalHeight) {
          canvas.width = img.naturalWidth;
          canvas.height = img.naturalHeight;
        }
        ctx.drawImage(img, 0, 0);
        resolve();
      };
      img.onerror = () => reject(new Error("frame decode failed"));
      img.src = url;
    });
  } finally {
    URL.revokeObjectURL(url);
  }
}

/**
 * FFmpeg-backed fallback player: streams decoded JPEG frames onto <canvas> and stereo PCM audio
 * into Web Audio when the webview's built-in player lacks codecs (for example missing GStreamer
 * plugins on Linux or ProRes/HEVC/MKV/FLAC files).
 */
function FfmpegMediaPlayer({
  path,
  kind,
  probe,
  big,
  mediaBase,
  initialPlaying,
  initialSeekSec = 0,
  onFatal,
}: {
  path: string;
  kind: "video" | "audio";
  probe: VideoProbe;
  big?: boolean;
  mediaBase: string;
  initialPlaying: boolean;
  initialSeekSec?: number;
  onFatal: () => void;
}) {
  const settings = useStore((s) => s.settings)!;
  const wrapRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const duration = probe.duration && Number.isFinite(probe.duration) && probe.duration > 0 ? probe.duration : 0;
  const hasVideo = probe.hasVideo ?? kind === "video";
  const hasAudio = probe.hasAudio ?? kind === "audio";
  const fps = choosePlaybackFps(probe.fps);
  const maxWidth = big ? 1280 : 720;
  const clampedStart = duration > 0 ? Math.max(0, Math.min(duration, initialSeekSec)) : Math.max(0, initialSeekSec);

  const [playing, setPlaying] = useState(initialPlaying);
  const [time, setTime] = useState(clampedStart);
  const [muted, setMuted] = useState(false);
  const [hasFrame, setHasFrame] = useState(false);
  const startAtRef = useRef(clampedStart);
  const timeRef = useRef(clampedStart);
  const mutedRef = useRef(false);
  const gainRef = useRef<GainNode | null>(null);
  const onFatalRef = useRef(onFatal);
  onFatalRef.current = onFatal;
  const [playNonce, setPlayNonce] = useState(0);
  const [frameNonce, setFrameNonce] = useState(clampedStart > 0 ? 1 : 0);

  useEffect(() => {
    mutedRef.current = muted;
    if (gainRef.current) gainRef.current.gain.value = muted ? 0 : 1;
  }, [muted]);

  useEffect(() => {
    if (big || initialPlaying) wrapRef.current?.focus();
  }, [big, initialPlaying, path]);

  // Draw a single paused frame on paused seek/step (or initial paused load when no poster was
  // cached). When a cached poster already exists at t=0, show it directly without spawning an
  // extra ffmpeg process.
  useEffect(() => {
    if (playing || !hasVideo) return;
    if (frameNonce === 0 && probe.poster) return;
    const ctl = new AbortController();
    const url = ffmpegFrameUrl(mediaBase, path, { timeSec: timeRef.current, maxWidth });
    fetch(url, { signal: ctl.signal })
      .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error(String(r.status)))))
      .then(async (buf) => {
        if (ctl.signal.aborted || !canvasRef.current) return;
        await drawJpegOnCanvas(canvasRef.current, new Uint8Array(buf));
        if (!ctl.signal.aborted) setHasFrame(true);
      })
      .catch(() => {});
    return () => ctl.abort();
  }, [frameNonce, path, hasVideo, mediaBase, maxWidth, playing, probe.poster]);

  // Stream video frames and/or stereo PCM audio while playing.
  useEffect(() => {
    if (!playing) return;
    const ctl = new AbortController();
    const startSec = startAtRef.current;
    let disposed = false;
    let rafId = 0;
    let audioCtx: AudioContext | null = null;
    let audioOrigin: number | null = null;
    let videoDone = !hasVideo;
    let audioDone = !hasAudio;
    let wallStart: number | null = null;
    let shownIdx = -1;
    let framesDelivered = 0;
    let drawing = false;
    const queue: Uint8Array[] = [];
    let wakeDrain: (() => void) | null = null;

    const finishIfDone = () => {
      if (disposed || !videoDone || !audioDone) return;
      const endSec = duration > 0 ? duration : timeRef.current;
      timeRef.current = endSec;
      startAtRef.current = endSec;
      setTime(endSec);
      setPlaying(false);
    };

    const elapsed = (): number => {
      // Only follow AudioContext when its clock is actually advancing: on Linux without
      // gst-plugins-good (`autoaudiosink`), WebKitGTK's AudioContext can stay stuck at 0.
      if (
        audioCtx &&
        audioOrigin !== null &&
        audioCtx.state === "running" &&
        audioCtx.currentTime > audioOrigin + 0.01
      ) {
        return Math.max(0, audioCtx.currentTime - audioOrigin);
      }
      if (wallStart === null) return 0;
      return Math.max(0, (performance.now() - wallStart) / 1000);
    };

    if (hasAudio && typeof AudioContext !== "undefined") {
      try {
        audioCtx = new AudioContext({ sampleRate: FFMPEG_AUDIO_RATE });
        const gain = audioCtx.createGain();
        gain.gain.value = mutedRef.current ? 0 : 1;
        gain.connect(audioCtx.destination);
        gainRef.current = gain;
        void audioCtx.resume().catch(() => {});
        const ctx = audioCtx;
        const audioUrl = ffmpegAudioUrl(mediaBase, path, startSec);
        void (async () => {
          try {
            const resp = await fetch(audioUrl, { signal: ctl.signal });
            if (!resp.ok || !resp.body) {
              audioDone = true;
              finishIfDone();
              return;
            }
            const reader = resp.body.getReader();
            const chunker = new PcmStereoChunker();
            let nextAt = 0;
            for (;;) {
              const { done, value } = await reader.read();
              if (disposed || ctl.signal.aborted) break;
              if (done) break;
              if (!value) continue;
              const pcm = chunker.push(value);
              if (!pcm) continue;
              const ab = ctx.createBuffer(FFMPEG_AUDIO_CHANNELS, pcm.left.length, FFMPEG_AUDIO_RATE);
              ab.getChannelData(0).set(pcm.left);
              ab.getChannelData(1).set(pcm.right);
              const now = ctx.currentTime;
              if (audioOrigin === null) {
                audioOrigin = now + 0.04;
                nextAt = audioOrigin;
                if (wallStart === null) wallStart = performance.now();
              } else if (nextAt < now - 0.1) {
                nextAt = now + 0.02;
              }
              const src = ctx.createBufferSource();
              src.buffer = ab;
              src.connect(gain);
              src.start(nextAt);
              nextAt += ab.duration;
              // Keep at most ~0.75 s buffered ahead when the audio clock is advancing.
              const waitStarted = performance.now();
              while (
                !disposed &&
                !ctl.signal.aborted &&
                nextAt - ctx.currentTime > 0.75 &&
                performance.now() - waitStarted < 1500
              ) {
                if (ctx.currentTime <= 0 && performance.now() - waitStarted > 300) break;
                await new Promise((r) => setTimeout(r, 80));
              }
            }
            if (!disposed && !ctl.signal.aborted && ctx.currentTime > 0 && nextAt > ctx.currentTime) {
              await new Promise((r) => setTimeout(r, Math.min(2000, (nextAt - ctx.currentTime) * 1000 + 30)));
            }
          } catch {
            // Aborted or stream ended.
          } finally {
            audioDone = true;
            finishIfDone();
          }
        })();
      } catch {
        audioDone = true;
      }
    } else {
      audioDone = true;
    }

    if (hasVideo) {
      const videoUrl = ffmpegVideoUrl(mediaBase, path, { startSec, fps, maxWidth });
      void (async () => {
        try {
          const resp = await fetch(videoUrl, { signal: ctl.signal });
          if (!resp.ok || !resp.body) {
            if (!disposed && !ctl.signal.aborted && framesDelivered === 0) onFatalRef.current();
            videoDone = true;
            finishIfDone();
            return;
          }
          const reader = resp.body.getReader();
          const parser = new MjpegFrameParser();
          for (;;) {
            const { done, value } = await reader.read();
            if (disposed || ctl.signal.aborted) break;
            if (done) break;
            if (!value) continue;
            for (const frame of parser.push(value)) {
              queue.push(frame);
              if (wallStart === null) wallStart = performance.now();
            }
            while (!disposed && !ctl.signal.aborted && queue.length >= 4) {
              await new Promise<void>((resolve) => { wakeDrain = resolve; });
            }
          }
          if (!disposed && !ctl.signal.aborted && framesDelivered === 0 && queue.length === 0) {
            onFatalRef.current();
          }
        } catch {
          // Aborted on pause/seek/unmount.
        } finally {
          videoDone = true;
          if (queue.length === 0) finishIfDone();
        }
      })();
    }

    const tick = () => {
      if (disposed) return;
      const el = elapsed();
      const cur = duration > 0 ? Math.min(duration, startSec + el) : startSec + el;
      if (wallStart !== null) {
        timeRef.current = cur;
        setTime(cur);
      }

      if (hasVideo && !drawing && queue.length > 0 && canvasRef.current) {
        const targetIdx = Math.floor(el * fps);
        if (shownIdx < 0 || targetIdx > shownIdx) {
          let frame = queue.shift()!;
          shownIdx++;
          // Drop stale buffered frames if decoding fell behind real time.
          while (queue.length > 0 && shownIdx < targetIdx) {
            frame = queue.shift()!;
            shownIdx++;
          }
          if (wakeDrain && queue.length < 4) {
            const w = wakeDrain;
            wakeDrain = null;
            w();
          }
          framesDelivered++;
          drawing = true;
          const canvas = canvasRef.current;
          void drawJpegOnCanvas(canvas, frame)
            .then(() => { if (!disposed) setHasFrame(true); })
            .catch(() => {})
            .finally(() => {
              drawing = false;
              if (videoDone && queue.length === 0) finishIfDone();
            });
        }
      } else if (hasVideo && videoDone && queue.length === 0 && !drawing) {
        finishIfDone();
      }
      rafId = requestAnimationFrame(tick);
    };
    rafId = requestAnimationFrame(tick);

    return () => {
      disposed = true;
      ctl.abort();
      cancelAnimationFrame(rafId);
      if (wakeDrain) {
        const w = wakeDrain;
        wakeDrain = null;
        w();
      }
      gainRef.current = null;
      if (audioCtx) void audioCtx.close().catch(() => {});
    };
  }, [playing, playNonce, path, hasVideo, hasAudio, fps, maxWidth, duration, mediaBase]);

  const seekTo = (nextSec: number, keepPlaying = playing) => {
    const clamped = duration > 0 ? Math.max(0, Math.min(duration, nextSec)) : Math.max(0, nextSec);
    timeRef.current = clamped;
    startAtRef.current = clamped;
    setTime(clamped);
    setPlaying(keepPlaying);
    if (keepPlaying) setPlayNonce((n) => n + 1);
    else setFrameNonce((n) => n + 1);
  };

  const togglePlay = () => {
    if (playing) {
      startAtRef.current = timeRef.current;
      setPlaying(false);
    } else {
      const start = duration > 0 && timeRef.current >= duration - 0.08 ? 0 : timeRef.current;
      timeRef.current = start;
      startAtRef.current = start;
      setTime(start);
      setPlaying(true);
      setPlayNonce((n) => n + 1);
    }
  };

  const stepSeek = (dir: number) => {
    const step = settings.videoSeek === "fraction" && duration > 0
      ? duration / Math.max(2, settings.videoSeekFraction)
      : settings.videoSeekSeconds;
    seekTo(timeRef.current + dir * step, playing);
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowRight") stepSeek(1);
    else if (e.key === "ArrowLeft") stepSeek(-1);
    else if (e.key === "]" && hasVideo) seekTo(timeRef.current + 1 / fps, false);
    else if (e.key === "[" && hasVideo) seekTo(timeRef.current - 1 / fps, false);
    else if (e.key === "k" || e.key === "K") togglePlay();
    else return;
    e.preventDefault();
    e.stopPropagation();
  };

  const codecLabel = formatCodecLabel(probe);

  return (
    <div className="pv-ff-player" ref={wrapRef} tabIndex={0} onKeyDown={onKey}>
      {hasVideo ? (
        <div className="pv-ff-stage" onClick={togglePlay}>
          {!hasFrame && probe.poster && (
            <img className="pv-ff-canvas" src={fileSrc(probe.poster)} draggable={false} />
          )}
          <canvas
            ref={canvasRef}
            className="pv-ff-canvas"
            style={hasFrame ? undefined : { position: "absolute", opacity: 0, pointerEvents: "none" }}
          />
        </div>
      ) : (
        <div className="pv-ff-audio-card">
          <Icon name="audio" size={56} />
          {codecLabel && <span className="pv-fb-meta">{codecLabel}</span>}
        </div>
      )}
      <div className="pv-ff-controls" onClick={(e) => e.stopPropagation()}>
        <button
          type="button"
          className="pv-ff-btn"
          title={playing ? tt("Pause (k)") : tt("Play (k)")}
          onClick={togglePlay}
        >
          {playing ? "❚❚" : "▶"}
        </button>
        <input
          type="range"
          className="pv-ff-scrub"
          min={0}
          max={duration > 0 ? duration : 1}
          step={0.05}
          value={Math.min(time, duration > 0 ? duration : 1)}
          disabled={duration <= 0}
          onChange={(e) => seekTo(Number(e.target.value), playing)}
          aria-label={tt("Seek")}
        />
        <span className="pv-ff-time">
          {formatMediaDuration(time)}{duration > 0 ? ` / ${formatMediaDuration(duration)}` : ""}
        </span>
        {hasAudio && (
          <button
            type="button"
            className="pv-ff-btn"
            title={muted ? tt("Unmute") : tt("Mute")}
            onClick={() => setMuted((m) => !m)}
          >
            {muted ? "🔇" : "🔊"}
          </button>
        )}
        <button
          type="button"
          className="pv-ff-btn"
          title={tt("Open in player")}
          aria-label={tt("Open in player")}
          onClick={() => api.openPath(path).catch(() => {})}
        >
          ↗
        </button>
      </div>
    </div>
  );
}

/**
 * Fallback card when neither the built-in player nor FFmpeg could play the file: shows a still
 * frame (if available), the codec and which packages to install, plus "Open in player".
 */
function MediaFallbackCard({
  path,
  kind,
  probe,
  probing,
  err,
}: {
  path: string;
  kind: "video" | "audio";
  probe: VideoProbe | null;
  probing: boolean;
  err: string;
}) {
  if (probing) {
    return (
      <div className="pv-icon">
        <Icon name={kind} size={56} />
        <span>{tt("Loading preview…")}</span>
      </div>
    );
  }

  const codec = formatCodecLabel(probe);
  const platform = detectPlatform(probe);
  const meta = [probe?.width && probe.height ? `${probe.width} × ${probe.height}` : "", probe?.duration ? formatMediaDuration(probe.duration) : ""].filter(Boolean).join(" · ");
  return (
    <div className="pv-fallback">
      {probe?.poster ? <img className="pv-poster" src={fileSrc(probe.poster)} draggable={false} /> : <Icon name={kind} size={56} />}
      <div className="pv-fb-card">
        <b>{codec ? `Can't play ${codec} here` : `Can't play this ${kind}`}</b>
        {meta && <span className="pv-fb-meta">{meta}</span>}
        <small>
          {platform === "linux"
            ? <>{tt("Playback uses the system's GStreamer codecs. Install")} <code>gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-libav</code> (Debian/Ubuntu), <code>gstreamer1-plugins-base gstreamer1-plugins-good gstreamer1-libav</code> (Fedora), <code>gst-plugins-base gst-plugins-good gst-libav</code> (Arch) or <code>ffmpeg</code>, then restart Gall.</>
            : platform === "windows"
              ? <>{tt("The built-in player (WebView2) only plays H.264, VP8/VP9 and AV1 video.")}</>
              : <>{tt("The built-in player cannot decode this format.")}</>}
          {!probe?.ffmpeg && <> {tt("Install FFmpeg to enable fallback playback and codec details here.")}</>}
        </small>
        <div className="pv-fb-actions">
          <button type="button" className="btn primary" onClick={() => api.openPath(path).catch(() => {})}>{tt("Open in player")}</button>
        </div>
        <small className="pv-fb-err" title={tt("Error from the built-in player")}>{err}</small>
      </div>
    </div>
  );
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
    const request = new AbortController();
    let loading: import("pdfjs-dist").PDFDocumentLoadingTask | undefined;
    setDoc(null);
    setErr(null);
    (async () => {
      try {
        const pdfjs = await import("pdfjs-dist");
        const worker = new URL("pdfjs-dist/build/pdf.worker.min.mjs", import.meta.url).href;
        if (!alive) return;
        pdfjs.GlobalWorkerOptions.workerSrc = worker;
        const response = await fetch(fileSrc(path), { signal: request.signal, cache: "no-store" });
        if (!response.ok) throw new Error(`PDF could not be read (${response.status} ${response.statusText})`);
        const data = await response.arrayBuffer();
        if (!alive) return;
        loading = pdfjs.getDocument({ data });
        const loaded = await loading.promise;
        if (alive) setDoc(loaded);
      } catch (e) {
        if (alive) setErr(String(e));
      }
    })();
    return () => {
      alive = false;
      request.abort();
      // The loading task owns the worker, including while parsing is still pending.
      void loading?.destroy().catch(() => {});
    };
  }, [path]);

  useEffect(() => {
    if (!doc || !canvas.current || !box.current) return;
    let task: any = null;
    let text: any = null;
    // A newer page / zoom started meanwhile: stop, so two renders never share the canvas
    // (pdf.js refuses that, and the older page could end up drawn over the newer one).
    let stale = false;
    (async () => {
      const p = await doc.getPage(page);
      if (stale || !canvas.current || !box.current) return;
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
      const rendered = task.promise.catch((e: unknown) => {
        if (!stale) setErr(String(e));
      });
      const tb = textBox.current;
      if (tb) {
        tb.replaceChildren();
        tb.style.setProperty("--total-scale-factor", String(scale));
        const { TextLayer } = await import("pdfjs-dist");
        if (stale) return;
        text = new TextLayer({ textContentSource: p.streamTextContent(), container: tb, viewport: p.getViewport({ scale }) });
        text.render().catch(() => {});
      }
      await rendered;
    })().catch((e) => {
      if (!stale) setErr(String(e));
    });
    return () => { stale = true; task?.cancel?.(); text?.cancel?.(); };
  }, [doc, page, fitWidth, zoom]);

  if (err) return <div className="pv-icon pv-pdf-error" title={err}><Icon name="pdf" size={56} /><span>{tt("PDF preview failed")}</span><small>{err}</small></div>;
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
          <button title={tt("Previous page (←)")} onClick={() => setPage((p) => Math.max(1, p - 1))}>‹</button>
          <span>{page} / {doc.numPages}</span>
          <button title={tt("Next page (→)")} onClick={() => setPage((p) => Math.min(doc.numPages, p + 1))}>›</button>
          <button title={tt("Zoom out (−, Ctrl+wheel)")} onClick={() => zoomBy(1 / 1.25)}>−</button>
          <button className="pct" title={tt("Back to fit (0)")} onClick={() => setZoom(1)}>{Math.round(zoom * 100)}%</button>
          <button title={tt("Zoom in (+, Ctrl+wheel)")} onClick={() => zoomBy(1.25)}>+</button>
          <button title={fitWidth ? "Fit the whole page in the window" : "Fit the page width to the window"} onClick={() => { setFitWidth((f) => !f); setZoom(1); }}>{fitWidth ? "Fit page" : "Fit width"}</button>
        </div>
      )}
    </div>
  );
}
