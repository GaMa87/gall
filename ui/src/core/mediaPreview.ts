// Pure decision and formatting helpers for zero-spool video/audio previews.
import type { VideoProbe } from "../api/types";
import { formatSize } from "./format";

/**
 * Largest Linux media file that native WebKitGTK `<video>`/`<audio>` may open without
 * asking first when FFmpeg is unavailable. WebKitGTK's GStreamer `downloadbuffer` spools
 * HTTP media to `/var/tmp/WebKit-Media-*` at disk speed, so multi-hundred-MB or multi-GB
 * files need explicit confirmation or external playback.
 */
export const LINUX_NATIVE_WARN_BYTES = 128 * 1024 * 1024;

export const CODEC_NAMES: Record<string, string> = {
  prores: "Apple ProRes",
  dnxhd: "Avid DNxHD/DNxHR",
  hevc: "H.265 / HEVC",
  h264: "H.264",
  cfhd: "GoPro CineForm",
  mpeg2video: "MPEG-2",
  mpeg4: "MPEG-4 Part 2",
  mjpeg: "Motion JPEG",
  av1: "AV1",
  vp9: "VP9",
  qtrle: "QuickTime Animation",
  flac: "FLAC",
  aac: "AAC",
  mp3: "MP3",
  opus: "Opus",
  vorbis: "Vorbis",
  pcm_s16le: "PCM",
};

export type MediaViewMode = "standby" | "large-guardrail" | "ffmpeg" | "native" | "fallback";

export interface MediaModeInput {
  activated: boolean;
  big: boolean;
  autoplayMedia: boolean;
  preferFfmpeg: boolean;
  probe: VideoProbe | null;
  probing: boolean;
  mediaBase: string | null;
  sizeBytes: number;
  isLinux: boolean;
  allowLargeNative: boolean;
  nativeErr: string | null;
  ffFailed: boolean;
}

export function canUseFfmpegPlayer(
  probe: VideoProbe | null,
  mediaBase: string | null,
  ffFailed: boolean,
): boolean {
  return !ffFailed && Boolean(probe?.ffmpeg && mediaBase && (probe.hasVideo || probe.hasAudio));
}

/**
 * Picks the preview rendering mode without mounting `<video>`/`<audio>` on hover/selection.
 *
 * - Before explicit play (unless opening the big preview window with `autoplayMedia`),
 *   stays in `"standby"` (poster + metadata + Play button; zero HTTP media traffic and zero
 *   `/var/tmp/WebKit-Media-*` spooling).
 * - On Linux when `preferFfmpeg` is set (missing GStreamer plugins OR file > 32 MiB with
 *   FFmpeg installed), streams directly through the zero-spool FFmpeg player.
 * - On Linux without FFmpeg, guards files larger than `LINUX_NATIVE_WARN_BYTES` behind an
 *   explicit confirmation card so multi-GB files are not silently copied into `/var/tmp`.
 */
export function decideMediaViewMode(input: MediaModeInput): MediaViewMode {
  const wantPlay = input.activated || (input.big && input.autoplayMedia);
  if (!wantPlay) return "standby";

  const canFfmpeg = canUseFfmpegPlayer(input.probe, input.mediaBase, input.ffFailed);

  if (input.preferFfmpeg) {
    if (canFfmpeg) return "ffmpeg";
    if (input.probing && !input.ffFailed) return "standby";
    return "fallback";
  }

  if (input.nativeErr) {
    if (canFfmpeg) return "ffmpeg";
    return "fallback";
  }

  if (input.isLinux && input.sizeBytes > LINUX_NATIVE_WARN_BYTES && !input.allowLargeNative) {
    if (canFfmpeg) return "ffmpeg";
    if (input.probing && !input.ffFailed) return "standby";
    return "large-guardrail";
  }

  return "native";
}

export function formatMediaDuration(seconds: number): string {
  const s = Math.max(0, Number.isFinite(seconds) ? seconds : 0);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = Math.floor(s % 60);
  return h
    ? `${h}:${String(m).padStart(2, "0")}:${String(sec).padStart(2, "0")}`
    : `${m}:${String(sec).padStart(2, "0")}`;
}

export function formatCodecLabel(probe: VideoProbe | null): string | null {
  if (!probe?.codec) return null;
  const base = CODEC_NAMES[probe.codec] ?? probe.codecLong ?? probe.codec;
  return probe.profile ? `${base} ${probe.profile}` : base;
}

export function formatMediaMeta(probe: VideoProbe | null, sizeBytes?: number): string {
  const parts: string[] = [];
  if (probe?.width && probe.height) parts.push(`${probe.width} × ${probe.height}`);
  const codec = formatCodecLabel(probe);
  if (codec) parts.push(codec);
  if (probe?.duration && Number.isFinite(probe.duration) && probe.duration > 0) {
    parts.push(formatMediaDuration(probe.duration));
  }
  if (typeof sizeBytes === "number" && Number.isFinite(sizeBytes) && sizeBytes > 0) {
    parts.push(formatSize(sizeBytes));
  }
  return parts.join(" · ");
}

export function detectPlatform(probe: VideoProbe | null): string {
  if (probe?.platform) return probe.platform;
  if (typeof navigator !== "undefined") {
    if (navigator.userAgent.includes("Windows")) return "windows";
    if (navigator.userAgent.includes("Mac")) return "macos";
  }
  return "linux";
}
