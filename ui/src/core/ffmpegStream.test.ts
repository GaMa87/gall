import { describe, expect, it } from "vitest";
import {
  MjpegFrameParser,
  PcmStereoChunker,
  choosePlaybackFps,
  ffmpegAudioUrl,
  ffmpegFrameUrl,
  ffmpegVideoUrl,
} from "./ffmpegStream";

const enc = new TextEncoder();

function mpjpegPart(payload: Uint8Array, withLength = true): Uint8Array {
  const head = enc.encode(
    `--gallframe\r\nContent-type: image/jpeg\r\n${withLength ? `Content-length: ${payload.length}\r\n` : ""}\r\n`,
  );
  const tail = enc.encode("\r\n");
  const out = new Uint8Array(head.length + payload.length + tail.length);
  out.set(head, 0);
  out.set(payload, head.length);
  out.set(tail, head.length + payload.length);
  return out;
}

describe("ffmpegStream", () => {
  it("builds loopback FFmpeg URLs with percent-encoded paths and clamped parameters", () => {
    const base = "http://127.0.0.1:41234/abc/";
    expect(ffmpegVideoUrl(base, "/home/u/My Clip.mp4", { startSec: 1.5, fps: 24, maxWidth: 960 }))
      .toBe("http://127.0.0.1:41234/abc/_ff/video/home/u/My%20Clip.mp4?t=1.500&fps=24&w=960");
    expect(ffmpegFrameUrl(base, "/home/u/č.mp4", { timeSec: -2, maxWidth: 4000 }))
      .toBe("http://127.0.0.1:41234/abc/_ff/frame/home/u/%C4%8D.mp4?t=0.000&w=1920");
    expect(ffmpegAudioUrl(base, "/home/u/a b.flac", 12.25))
      .toBe("http://127.0.0.1:41234/abc/_ff/audio/home/u/a%20b.flac?t=12.250");
  });

  it("chooses sensible playback FPS", () => {
    expect(choosePlaybackFps(undefined)).toBe(24);
    expect(choosePlaybackFps(570 / 23)).toBe(24);
    expect(choosePlaybackFps(60)).toBe(24);
    expect(choosePlaybackFps(15)).toBe(15);
    expect(choosePlaybackFps(2)).toBe(6);
  });

  it("extracts multipart JPEG frames across arbitrary chunk splits", () => {
    const f1 = new Uint8Array([0xff, 0xd8, 1, 2, 3, 0xff, 0xd9]);
    const f2 = new Uint8Array([0xff, 0xd8, 9, 8, 7, 6, 0xff, 0xd9]);
    const p1 = mpjpegPart(f1, true);
    const p2 = mpjpegPart(f2, true);
    const stream = new Uint8Array(p1.length + p2.length);
    stream.set(p1, 0);
    stream.set(p2, p1.length);

    const parser = new MjpegFrameParser();
    const got: Uint8Array[] = [];
    // Feed 5 bytes at a time to split boundaries, headers and payloads.
    for (let i = 0; i < stream.length; i += 5) {
      got.push(...parser.push(stream.subarray(i, Math.min(stream.length, i + 5))));
    }
    expect(got).toHaveLength(2);
    expect(Array.from(got[0])).toEqual(Array.from(f1));
    expect(Array.from(got[1])).toEqual(Array.from(f2));
  });

  it("extracts frames delimited by boundary when Content-Length is absent", () => {
    const f1 = new Uint8Array([0xff, 0xd8, 42, 0xff, 0xd9]);
    const f2 = new Uint8Array([0xff, 0xd8, 99, 0xff, 0xd9]);
    const p1 = mpjpegPart(f1, false);
    const p2 = mpjpegPart(f2, false);
    const stream = new Uint8Array(p1.length + p2.length);
    stream.set(p1, 0);
    stream.set(p2, p1.length);

    const parser = new MjpegFrameParser();
    const got = parser.push(stream);
    expect(got).toHaveLength(1);
    expect(Array.from(got[0])).toEqual(Array.from(f1));
  });

  it("de-interleaves stereo f32le PCM across unaligned chunk boundaries", () => {
    const raw = new ArrayBuffer(16);
    const dv = new DataView(raw);
    dv.setFloat32(0, 0.25, true);
    dv.setFloat32(4, -0.5, true);
    dv.setFloat32(8, 0.75, true);
    dv.setFloat32(12, 1.0, true);
    const bytes = new Uint8Array(raw);

    const chunker = new PcmStereoChunker();
    expect(chunker.push(bytes.subarray(0, 5))).toBeNull();
    const first = chunker.push(bytes.subarray(5, 11));
    expect(first).not.toBeNull();
    expect(Array.from(first!.left)).toEqual([0.25]);
    expect(Array.from(first!.right)).toEqual([-0.5]);
    const second = chunker.push(bytes.subarray(11, 16));
    expect(second).not.toBeNull();
    expect(Array.from(second!.left)).toEqual([0.75]);
    expect(Array.from(second!.right)).toEqual([1.0]);
  });
});
