// Pure helpers for streaming FFmpeg-decoded video frames (multipart JPEG) and raw stereo PCM
// audio from the local loopback media server when the webview's built-in codecs are unavailable.

export const FFMPEG_AUDIO_RATE = 44100;
export const FFMPEG_AUDIO_CHANNELS = 2;
const BOUNDARY_TEXT = "--gallframe";
const MAX_BUFFER_BYTES = 12 * 1024 * 1024;

function encodePath(path: string): string {
  return path.split("/").map(encodeURIComponent).join("/").replace(/^\//, "");
}

/** Picks a smooth, CPU-friendly playback FPS for the canvas fallback player. */
export function choosePlaybackFps(sourceFps: number | null | undefined): number {
  if (!sourceFps || !Number.isFinite(sourceFps) || sourceFps <= 0) return 24;
  return Math.max(6, Math.min(24, Math.round(sourceFps)));
}

export function ffmpegVideoUrl(
  mediaBase: string,
  path: string,
  opts: { startSec: number; fps: number; maxWidth: number },
): string {
  const t = Math.max(0, Number.isFinite(opts.startSec) ? opts.startSec : 0).toFixed(3);
  const fps = Math.max(1, Math.min(30, Math.round(opts.fps)));
  const w = Math.max(64, Math.min(1920, Math.round(opts.maxWidth)));
  return `${mediaBase}_ff/video/${encodePath(path)}?t=${t}&fps=${fps}&w=${w}`;
}

export function ffmpegFrameUrl(
  mediaBase: string,
  path: string,
  opts: { timeSec: number; maxWidth: number },
): string {
  const t = Math.max(0, Number.isFinite(opts.timeSec) ? opts.timeSec : 0).toFixed(3);
  const w = Math.max(64, Math.min(1920, Math.round(opts.maxWidth)));
  return `${mediaBase}_ff/frame/${encodePath(path)}?t=${t}&w=${w}`;
}

export function ffmpegAudioUrl(mediaBase: string, path: string, startSec: number): string {
  const t = Math.max(0, Number.isFinite(startSec) ? startSec : 0).toFixed(3);
  return `${mediaBase}_ff/audio/${encodePath(path)}?t=${t}`;
}

function indexOfBytes(haystack: Uint8Array, needle: Uint8Array, from = 0): number {
  if (needle.length === 0 || haystack.length - from < needle.length) return -1;
  outer: for (let i = from; i <= haystack.length - needle.length; i++) {
    for (let j = 0; j < needle.length; j++) {
      if (haystack[i + j] !== needle[j]) continue outer;
    }
    return i;
  }
  return -1;
}

const ASCII = new TextDecoder("ascii");
const BOUNDARY_BYTES = new TextEncoder().encode(BOUNDARY_TEXT);
const HEADER_END_CRLF = new Uint8Array([13, 10, 13, 10]);
const HEADER_END_LF = new Uint8Array([10, 10]);

/**
 * Incrementally extracts JPEG frames from an `ffmpeg -f mpjpeg -boundary_tag gallframe` stream,
 * handling arbitrary chunk splits and case-insensitive `Content-Length` headers.
 */
export class MjpegFrameParser {
  private buf: Uint8Array = new Uint8Array(0);

  push(chunk: Uint8Array): Uint8Array[] {
    if (chunk.length === 0) return [];
    const merged = new Uint8Array(this.buf.length + chunk.length);
    merged.set(this.buf, 0);
    merged.set(chunk, this.buf.length);
    this.buf = merged;
    if (this.buf.length > MAX_BUFFER_BYTES) {
      // Drop oldest data up to the last boundary so a corrupt stream cannot grow unboundedly.
      const last = indexOfBytes(this.buf, BOUNDARY_BYTES, Math.max(0, this.buf.length - 1024 * 1024));
      this.buf = last >= 0 ? this.buf.slice(last) : new Uint8Array(0);
    }

    const frames: Uint8Array[] = [];
    for (;;) {
      const bStart = indexOfBytes(this.buf, BOUNDARY_BYTES, 0);
      if (bStart < 0) {
        // Keep a short tail in case the boundary itself is split across chunks.
        if (this.buf.length > BOUNDARY_BYTES.length) {
          this.buf = this.buf.slice(this.buf.length - BOUNDARY_BYTES.length);
        }
        break;
      }
      if (bStart > 0) {
        this.buf = this.buf.slice(bStart);
      }

      let sepLen = 4;
      let headerEnd = indexOfBytes(this.buf, HEADER_END_CRLF, BOUNDARY_BYTES.length);
      const lfEnd = indexOfBytes(this.buf, HEADER_END_LF, BOUNDARY_BYTES.length);
      if (lfEnd >= 0 && (headerEnd < 0 || lfEnd < headerEnd)) {
        headerEnd = lfEnd;
        sepLen = 2;
      }
      if (headerEnd < 0) break;

      const headerText = ASCII.decode(this.buf.subarray(BOUNDARY_BYTES.length, headerEnd));
      const bodyStart = headerEnd + sepLen;
      const lenMatch = /content-length:\s*(\d+)/i.exec(headerText);
      if (lenMatch) {
        const len = Number(lenMatch[1]);
        if (!Number.isFinite(len) || len < 0 || len > MAX_BUFFER_BYTES) {
          this.buf = this.buf.slice(bodyStart);
          continue;
        }
        if (this.buf.length < bodyStart + len) break;
        if (len > 0) frames.push(this.buf.slice(bodyStart, bodyStart + len));
        this.buf = this.buf.slice(bodyStart + len);
        continue;
      }

      // Fallback when Content-Length is absent: frame ends at the next boundary.
      const nextBoundary = indexOfBytes(this.buf, BOUNDARY_BYTES, bodyStart);
      if (nextBoundary < 0) break;
      let bodyEnd = nextBoundary;
      while (bodyEnd > bodyStart && (this.buf[bodyEnd - 1] === 10 || this.buf[bodyEnd - 1] === 13)) {
        bodyEnd--;
      }
      if (bodyEnd > bodyStart) frames.push(this.buf.slice(bodyStart, bodyEnd));
      this.buf = this.buf.slice(nextBoundary);
    }
    return frames;
  }
}

/**
 * De-interleaves arbitrary byte chunks of stereo 32-bit little-endian float PCM (`f32le`) into
 * planar left/right `Float32Array` channels, carrying any 1..7 leftover bytes to the next chunk.
 */
export class PcmStereoChunker {
  private tail: Uint8Array = new Uint8Array(0);

  push(chunk: Uint8Array): { left: Float32Array; right: Float32Array } | null {
    if (chunk.length === 0) return null;
    const total = this.tail.length + chunk.length;
    const frameBytes = 8; // 2 channels * 4 bytes (f32le)
    const usable = total - (total % frameBytes);
    const merged = new Uint8Array(total);
    merged.set(this.tail, 0);
    merged.set(chunk, this.tail.length);

    this.tail = usable < total ? merged.slice(usable) : new Uint8Array(0);
    if (usable === 0) return null;

    const frames = usable / frameBytes;
    const left = new Float32Array(frames);
    const right = new Float32Array(frames);
    const view = new DataView(merged.buffer, merged.byteOffset, usable);
    for (let i = 0; i < frames; i++) {
      left[i] = view.getFloat32(i * 8, true);
      right[i] = view.getFloat32(i * 8 + 4, true);
    }
    return { left, right };
  }
}
