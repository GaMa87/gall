import { afterEach, describe, expect, it, vi } from "vitest";
import { MAX_MEDIA_BYTES, recordGall, recordingMime, validateMedia } from "./reportMedia";

describe("local report attachments", () => {
  it("accepts only bounded image and video types", () => {
    expect(validateMedia(new Blob(["pixels"], { type: "image/png" }))).toBe("image");
    expect(validateMedia(new Blob(["frames"], { type: "video/webm;codecs=vp8" }))).toBe("video");
    expect(() => validateMedia(new Blob([], { type: "image/png" }))).toThrow("non-empty");
    expect(() => validateMedia(new Blob(["<svg/>"], { type: "image/svg+xml" }))).toThrow("PNG");
    expect(() => validateMedia(new Blob([new Uint8Array(MAX_MEDIA_BYTES + 1)], { type: "video/mp4" }))).toThrow("8 MiB");
  });
  it("uses an available encoder instead of assuming every webview supports WebM", () => {
    expect(recordingMime((mime) => mime === "video/mp4")).toBe("video/mp4");
    expect(recordingMime((mime) => mime === "video/webm;codecs=vp8")).toBe("video/webm;codecs=vp8");
    expect(recordingMime(() => false)).toBeUndefined();
  });
});

describe("Gall-only recorder lifecycle", () => {
  const stopTrack = vi.fn();
  let latest: FakeRecorder;
  class FakeCanvas {
    width = 0; height = 0;
    getContext() { return { drawImage: vi.fn(), clearRect: vi.fn() }; }
    captureStream() { return { getTracks: () => [{ stop: stopTrack }] }; }
  }
  class FakeRecorder {
    static isTypeSupported(mime: string) { return mime.startsWith("video/webm"); }
    state = "inactive";
    mimeType = "video/webm;codecs=vp8";
    ondataavailable?: (event: { data: Blob }) => void;
    onstop?: () => void;
    onerror?: () => void;
    constructor() { latest = this; }
    start() { this.state = "recording"; }
    stop() {
      this.state = "inactive";
      this.ondataavailable?.({ data: new Blob(["video"]) });
      this.onstop?.();
    }
  }
  function setup() {
    vi.useFakeTimers(); stopTrack.mockClear();
    vi.stubGlobal("HTMLCanvasElement", FakeCanvas);
    vi.stubGlobal("document", { createElement: () => new FakeCanvas() });
    vi.stubGlobal("Image", class { naturalWidth = 100; naturalHeight = 80; decode() { return Promise.resolve(); } });
    vi.stubGlobal("MediaRecorder", FakeRecorder);
  }
  afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });
  it("stops tracks and emits one silent clip when stopped twice", async () => {
    setup();
    const done = vi.fn(), capture = vi.fn(async () => new Blob(["png"]));
    const recording = await recordGall(capture, done);
    recording.stop(); recording.stop();
    expect(done).toHaveBeenCalledTimes(1);
    expect(done.mock.calls[0][0].type).toBe("video/webm");
    expect(stopTrack).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(1000);
    expect(capture).toHaveBeenCalledTimes(1);
  });
  it("discards a cancelled recording and stops at the duration boundary", async () => {
    setup();
    const done = vi.fn();
    const recording = await recordGall(async () => new Blob(["png"]), done);
    recording.stop(true);
    expect(done).toHaveBeenCalledWith(undefined, undefined);
    const timed = vi.fn();
    await recordGall(async () => new Blob(["png"]), timed);
    await vi.advanceTimersByTimeAsync(20_000);
    expect(timed).toHaveBeenCalledTimes(1);
    expect(stopTrack).toHaveBeenCalledTimes(2);
  });
  it("does not keep a clip that exceeds the attachment limit", async () => {
    setup();
    const done = vi.fn();
    await recordGall(async () => new Blob(["png"]), done);
    latest.ondataavailable?.({ data: new Blob([new Uint8Array(MAX_MEDIA_BYTES + 1)]) });
    expect(done.mock.calls[0][0]).toBeUndefined();
    expect(done.mock.calls[0][1]).toContain("8 MiB");
    expect(stopTrack).toHaveBeenCalledOnce();
  });
  it("ends capture and explains snapshot failures", async () => {
    setup();
    const done = vi.fn(), capture = vi.fn().mockResolvedValueOnce(new Blob(["png"])).mockRejectedValue(new Error("Window unavailable"));
    await recordGall(capture, done);
    await vi.advanceTimersByTimeAsync(300);
    expect(done).toHaveBeenCalledWith(undefined, expect.stringContaining("Window unavailable"));
    expect(stopTrack).toHaveBeenCalledOnce();
  });
});
