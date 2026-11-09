import { normalizedBox, type Mark } from "./bugReport";

export const MAX_MEDIA_BYTES = 8 * 1024 * 1024;
export const RECORD_SECONDS = 20;
export interface ReportMedia { id: string; name: string; blob: Blob; kind: "image" | "video" }

export function validateMedia(blob: Blob): "image" | "video" {
  if (!blob.size || blob.size > MAX_MEDIA_BYTES) throw new Error("Choose a non-empty image or video no larger than 8 MiB.");
  if (["image/png", "image/jpeg", "image/webp"].includes(blob.type)) return "image";
  if (["video/mp4", "video/webm"].includes(blob.type.split(";")[0])) return "video";
  throw new Error("Choose a PNG, JPEG, WebP, MP4 or WebM file.");
}

export async function imageCanvas(blob: Blob): Promise<HTMLCanvasElement> {
  const url = URL.createObjectURL(blob);
  try {
    const img = new Image();
    img.src = url;
    await img.decode();
    const { naturalWidth: width, naturalHeight: height } = img;
    if (!width || !height || width > 8192 || height > 8192 || width * height > 16_777_216) throw new Error("Choose an image up to 8192 pixels per side and 16 million pixels total.");
    const canvas = document.createElement("canvas");
    canvas.width = width; canvas.height = height;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("Image editing is unavailable in this webview.");
    ctx.drawImage(img, 0, 0);
    return canvas;
  } finally { URL.revokeObjectURL(url); }
}

export function pngBlob(canvas: HTMLCanvasElement): Promise<Blob> {
  return new Promise((resolve, reject) => canvas.toBlob((blob) => {
    if (!blob) reject(new Error("Could not encode the screenshot."));
    else if (blob.size > MAX_MEDIA_BYTES) reject(new Error("The edited PNG exceeds 8 MiB. Use a smaller image."));
    else resolve(blob);
  }, "image/png"));
}

export async function flattenScreenshot(blob: Blob, marks: Mark[]): Promise<Blob> {
  const canvas = await imageCanvas(blob), ctx = canvas.getContext("2d")!;
  const w = canvas.width, h = canvas.height;
  ctx.lineWidth = Math.max(3, w / 350);
  ctx.strokeStyle = "#ff574f";
  for (const mark of marks) {
    const box = normalizedBox(mark);
    if (mark.tool === "redact") {
      ctx.fillStyle = "#111111";
      // Round outward so no subpixel edge can reveal covered source pixels.
      ctx.fillRect(Math.floor(box.x * w), Math.floor(box.y * h), Math.ceil((box.x + box.width) * w) - Math.floor(box.x * w), Math.ceil((box.y + box.height) * h) - Math.floor(box.y * h));
    } else if (mark.tool === "rectangle") ctx.strokeRect(box.x * w, box.y * h, box.width * w, box.height * h);
    else {
      const x = mark.x * w, y = mark.y * h, endX = mark.endX * w, endY = mark.endY * h;
      const angle = Math.atan2(endY - y, endX - x), head = Math.max(14, w / 55);
      ctx.beginPath(); ctx.moveTo(x, y); ctx.lineTo(endX, endY);
      ctx.moveTo(endX - head * Math.cos(angle - Math.PI / 6), endY - head * Math.sin(angle - Math.PI / 6));
      ctx.lineTo(endX, endY); ctx.lineTo(endX - head * Math.cos(angle + Math.PI / 6), endY - head * Math.sin(angle + Math.PI / 6)); ctx.stroke();
    }
  }
  return pngBlob(canvas);
}

export function recordingMime(supported: (mime: string) => boolean): string | undefined {
  return ["video/mp4;codecs=avc1.42E01E", "video/webm;codecs=vp8", "video/mp4", "video/webm"].find(supported);
}

/** Records only snapshots of Gall, never the desktop, microphone or other applications. */
export async function recordGall(capture: () => Promise<Blob>, done: (blob: Blob | undefined, error?: string) => void): Promise<{ stop: (discard?: boolean) => void }> {
  if (typeof MediaRecorder === "undefined" || !HTMLCanvasElement.prototype.captureStream) throw new Error("Recording is unavailable in this webview. Add an existing MP4 or WebM instead.");
  const mimeType = recordingMime((mime) => MediaRecorder.isTypeSupported(mime));
  if (!mimeType) throw new Error("No supported video encoder. Add an existing MP4 or WebM instead.");
  const first = await imageCanvas(await capture());
  const canvas = document.createElement("canvas"), scale = Math.min(1, 1280 / first.width, 800 / first.height);
  canvas.width = Math.max(2, Math.floor(first.width * scale / 2) * 2);
  canvas.height = Math.max(2, Math.floor(first.height * scale / 2) * 2);
  const ctx = canvas.getContext("2d")!;
  ctx.drawImage(first, 0, 0, canvas.width, canvas.height);
  const stream = canvas.captureStream(4);
  let recorder: MediaRecorder;
  try { recorder = new MediaRecorder(stream, { mimeType, videoBitsPerSecond: 1_000_000 }); }
  catch (error) { stream.getTracks().forEach((track) => track.stop()); throw error; }
  const chunks: Blob[] = [];
  let size = 0, ended = false, discard = false, failure: string | undefined;
  let timer: ReturnType<typeof setTimeout>;
  const cleanup = () => { clearTimeout(timer); clearTimeout(limit); stream.getTracks().forEach((track) => track.stop()); };
  const stop = (cancel = false) => {
    discard ||= cancel;
    if (ended) return;
    ended = true;
    clearTimeout(timer);
    if (recorder.state !== "inactive") recorder.stop();
  };
  recorder.ondataavailable = (event) => {
    size += event.data.size;
    if (size > MAX_MEDIA_BYTES) { failure = "The recording exceeded 8 MiB. Record a shorter clip."; stop(); }
    else if (event.data.size) chunks.push(event.data);
  };
  recorder.onerror = () => { failure = "Recording failed. Try a shorter clip or add an existing video."; stop(); };
  recorder.onstop = () => {
    cleanup();
    const blob = new Blob(chunks, { type: recorder.mimeType.split(";")[0] });
    done(discard || failure || !blob.size ? undefined : blob, failure || (!discard && !blob.size ? "The recording was empty. Try again." : undefined));
  };
  const limit = setTimeout(() => stop(), RECORD_SECONDS * 1000);
  try { recorder.start(250); } catch (error) { cleanup(); throw error; }
  const frame = async () => {
    try {
      const image = await imageCanvas(await capture());
      if (ended) return;
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
      timer = setTimeout(() => void frame(), 250);
    } catch (error) { if (!ended) { failure = `Capture stopped: ${String(error)}`; stop(); } }
  };
  timer = setTimeout(() => void frame(), 250);
  return { stop };
}
