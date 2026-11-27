import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { formatSize } from "../core/format";
import { imageByteLimitMessage } from "../core/imageCompare";
import type { GitImage, ImageSide } from "../core/gitImages";
import { fileSrc } from "../api/ipc";
import { tt } from "../i18n";
import { ImageCompare } from "./ImageCompare";
import "./GitImageDiff.css";

/** Before and after of a changed image. Nothing is read before the file is selected; `revision` reads it again. */
export function GitImageDiff({ path, sides, revision }: { path: string; sides: [ImageSide, ImageSide]; revision?: unknown }) {
  const key = JSON.stringify(sides);
  const [loaded, setLoaded] = useState<{ key: string; images: [GitImage, GitImage]; at: number } | null>(null), [error, setError] = useState("");
  useEffect(() => {
    let alive = true;
    setError("");
    const [left, right] = JSON.parse(key) as [ImageSide, ImageSide];
    // Moving over the files quickly reads only the image one stays on.
    const timer = setTimeout(() => {
      Promise.all([left, right].map((side) => invoke<GitImage>("git_image", { path, side }))).then(([a, b]) => { if (alive) setLoaded({ key, images: [a, b], at: Date.now() }); }).catch((e) => { if (alive) setError(String(e)); });
    }, 120);
    return () => { alive = false; clearTimeout(timer); };
  }, [path, key, revision]);
  if (error) return <p className="gcd-error" role="alert">{error}</p>;
  if (loaded?.key !== key) return <p className="gcd-notice" role="status">{tt("Loading images…")}</p>;
  const [a, b] = loaded.images;
  const reason = a.reason ?? b.reason ?? imageByteLimitMessage(a.size) ?? imageByteLimitMessage(b.size);
  return <div className="gid">
    <p className="gid-sizes">{a.path || a.size ? formatSize(a.size) : tt("no image")} → {b.path || b.size ? formatSize(b.size) : tt("no image")}</p>
    {reason ? <p className="gcd-notice">{tt(reason)}</p>
      : a.path && b.path ? <ImageCompare key={`${a.path}|${b.path}|${loaded.at}`} left={a.path} right={b.path} leftSize={a.size} rightSize={b.size} stamp={loaded.at} embedded labels={[tt("Before"), tt("After")]} />
        : a.path || b.path ? <figure className="gid-single ic-alpha"><img src={`${fileSrc((a.path ?? b.path)!)}?v=${loaded.at}`} draggable={false} alt="" /><figcaption>{a.path ? tt("Deleted") : tt("Added")}</figcaption></figure> : null}
  </div>;
}
