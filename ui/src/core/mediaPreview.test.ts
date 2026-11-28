import { describe, expect, it } from "vitest";
import type { VideoProbe } from "../api/types";
import {
  LINUX_NATIVE_WARN_BYTES,
  decideMediaViewMode,
  formatCodecLabel,
  formatMediaDuration,
  formatMediaMeta,
  type MediaModeInput,
} from "./mediaPreview";

const baseProbe = (overrides: Partial<VideoProbe> = {}): VideoProbe => ({
  platform: "linux",
  ffmpeg: true,
  codec: "hevc",
  codecLong: "H.265 / HEVC",
  profile: "Main 10",
  width: 3840,
  height: 2160,
  duration: 3725,
  fps: 24,
  hasVideo: true,
  hasAudio: true,
  poster: "/cache/posters/abc.jpg",
  ...overrides,
});

const baseInput = (overrides: Partial<MediaModeInput> = {}): MediaModeInput => ({
  activated: false,
  big: false,
  autoplayMedia: false,
  preferFfmpeg: false,
  probe: baseProbe(),
  probing: false,
  mediaBase: "http://127.0.0.1:1234/tok/",
  sizeBytes: 5 * 1024 * 1024,
  isLinux: true,
  allowLargeNative: false,
  nativeErr: null,
  ffFailed: false,
  ...overrides,
});

describe("decideMediaViewMode", () => {
  it("stays in zero-spool standby on hover/selection even for 20 GB files", () => {
    expect(
      decideMediaViewMode(
        baseInput({
          sizeBytes: 20 * 1024 * 1024 * 1024,
          preferFfmpeg: true,
          autoplayMedia: true, // details pane (big: false) must never autoplay
        }),
      ),
    ).toBe("standby");

    expect(
      decideMediaViewMode(
        baseInput({
          big: true,
          autoplayMedia: false,
          sizeBytes: 10 * 1024 * 1024,
        }),
      ),
    ).toBe("standby");
  });

  it("routes large or unsupported Linux media straight to the zero-spool FFmpeg player on play", () => {
    expect(
      decideMediaViewMode(
        baseInput({
          activated: true,
          preferFfmpeg: true,
          sizeBytes: 4 * 1024 * 1024 * 1024,
        }),
      ),
    ).toBe("ffmpeg");

    // Waits on standby while probing instead of flashing an error or mounting GStreamer.
    expect(
      decideMediaViewMode(
        baseInput({
          activated: true,
          preferFfmpeg: true,
          probe: null,
          probing: true,
        }),
      ),
    ).toBe("standby");

    // Falls back to the help card if FFmpeg playback itself failed.
    expect(
      decideMediaViewMode(
        baseInput({
          activated: true,
          preferFfmpeg: true,
          ffFailed: true,
        }),
      ),
    ).toBe("fallback");
  });

  it("uses native <video>/<audio> for small clips after explicit play, falling back to FFmpeg on codec error", () => {
    expect(decideMediaViewMode(baseInput({ activated: true }))).toBe("native");
    expect(
      decideMediaViewMode(
        baseInput({
          activated: true,
          nativeErr: "format or codec not supported",
        }),
      ),
    ).toBe("ffmpeg");
    expect(
      decideMediaViewMode(
        baseInput({
          activated: true,
          nativeErr: "format or codec not supported",
          probe: baseProbe({ ffmpeg: false }),
        }),
      ),
    ).toBe("fallback");
  });

  it("guards large Linux files when FFmpeg is absent until explicitly confirmed", () => {
    const noFfmpeg = baseProbe({ ffmpeg: false, codec: null, poster: null });
    expect(
      decideMediaViewMode(
        baseInput({
          activated: true,
          probe: noFfmpeg,
          sizeBytes: LINUX_NATIVE_WARN_BYTES + 1,
        }),
      ),
    ).toBe("large-guardrail");

    expect(
      decideMediaViewMode(
        baseInput({
          activated: true,
          probe: noFfmpeg,
          sizeBytes: LINUX_NATIVE_WARN_BYTES + 1,
          allowLargeNative: true,
        }),
      ),
    ).toBe("native");

    // Non-Linux (macOS / Windows asset://) does not trigger the Linux /var/tmp guardrail.
    expect(
      decideMediaViewMode(
        baseInput({
          activated: true,
          probe: noFfmpeg,
          isLinux: false,
          sizeBytes: 4 * 1024 * 1024 * 1024,
        }),
      ),
    ).toBe("native");
  });

  it("autoplays in the big preview window when autoplayMedia is enabled, still respecting FFmpeg routing", () => {
    expect(
      decideMediaViewMode(
        baseInput({
          big: true,
          autoplayMedia: true,
          preferFfmpeg: true,
          sizeBytes: 2 * 1024 * 1024 * 1024,
        }),
      ),
    ).toBe("ffmpeg");
  });
});

describe("media metadata formatting", () => {
  it("formats durations, codec labels, and metadata summaries", () => {
    expect(formatMediaDuration(0)).toBe("0:00");
    expect(formatMediaDuration(75)).toBe("1:15");
    expect(formatMediaDuration(3725)).toBe("1:02:05");
    expect(formatCodecLabel(baseProbe())).toBe("H.265 / HEVC Main 10");
    expect(formatMediaMeta(baseProbe(), 1536 * 1024 * 1024)).toContain("3840 × 2160 · H.265 / HEVC Main 10 · 1:02:05");
  });
});
