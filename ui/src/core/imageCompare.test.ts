import { describe, expect, it } from "vitest";
import {
  canPixelDiff,
  computePixelDiffMask,
  fitImagePair,
  formatDiffSummary,
  hasAlphaTransparency,
  imageByteLimitMessage,
  imagePixelLimitMessage,
  isImagePath,
  MAX_IMAGE_BYTES,
  MAX_IMAGE_PIXELS,
} from "./imageCompare";

describe("shared image compare helpers (disk + Git parity)", () => {
  it("classifies image extensions and pixel-diff support consistently", () => {
    expect(isImagePath("/tmp/a.PNG")).toBe(true);
    expect(isImagePath("/tmp/a.svg")).toBe(true);
    expect(isImagePath("/tmp/a.rs")).toBe(false);
    expect(canPixelDiff("a.png", "b.webp")).toBe(true);
    expect(canPixelDiff("a.svg", "b.svg")).toBe(false);
    expect(canPixelDiff("a.png", "b.avif")).toBe(false);
  });

  it("enforces identical byte and pixel limits for both adapters", () => {
    expect(imageByteLimitMessage(MAX_IMAGE_BYTES)).toBeNull();
    expect(imageByteLimitMessage(MAX_IMAGE_BYTES + 1)).toContain("24 MiB");
    expect(imagePixelLimitMessage({ width: 6000, height: 10000 })).toBeNull();
    expect(imagePixelLimitMessage({ width: 10000, height: 6001 })).toContain("60 megapixels");
    expect(MAX_IMAGE_PIXELS).toBe(60_000_000);
  });

  it("scales unequal dimensions onto a shared coordinate frame without distortion", () => {
    const layout = fitImagePair({ width: 200, height: 100 }, { width: 100, height: 200 })!;
    expect(layout.sizeMismatch).toBe(true);
    expect(layout.stageWidth).toBe(200);
    expect(layout.stageHeight).toBe(200);
    expect(layout.leftBox).toEqual({ widthPct: 100, heightPct: 50 });
    expect(layout.rightBox).toEqual({ widthPct: 50, heightPct: 100 });
    expect(layout.overlap).toEqual({ width: 100, height: 100 });

    const equal = fitImagePair({ width: 64, height: 64 }, { width: 64, height: 64 })!;
    expect(equal.sizeMismatch).toBe(false);
    expect(equal.leftBox).toEqual({ widthPct: 100, heightPct: 100 });
    expect(equal.rightBox).toEqual({ widthPct: 100, heightPct: 100 });
  });

  it("detects alpha transparency and differences in the alpha channel", () => {
    const opaque = new Uint8ClampedArray([10, 20, 30, 255, 10, 20, 30, 255]);
    const translucent = new Uint8ClampedArray([10, 20, 30, 255, 10, 20, 30, 128]);
    expect(hasAlphaTransparency(opaque)).toBe(false);
    expect(hasAlphaTransparency(translucent)).toBe(true);

    const diff = computePixelDiffMask(opaque, { width: 2, height: 1 }, translucent, { width: 2, height: 1 });
    expect(diff.compared).toBe(2);
    expect(diff.changed).toBe(1);
    expect(diff.maxDelta).toBe(127);
    expect(diff.extraPixels).toBe(0);
    // Second pixel highlighted in red; first pixel faded grey context.
    expect(Array.from(diff.mask.slice(4, 8))).toEqual([235, 40, 70, 255]);
    expect(diff.mask[0]).toBe(diff.mask[1]);
    expect(diff.mask[1]).toBe(diff.mask[2]);
  });

  it("counts extra pixels and reports overlap when image dimensions differ", () => {
    const left = new Uint8ClampedArray(2 * 2 * 4).fill(255);
    const right = new Uint8ClampedArray(3 * 2 * 4).fill(255);
    const mask = computePixelDiffMask(left, { width: 2, height: 2 }, right, { width: 3, height: 2 });
    expect(mask.compared).toBe(4);
    expect(mask.changed).toBe(0);
    expect(mask.extraPixels).toBe(2);

    const summary = formatDiffSummary({ changed: 0, compared: 4, leftSize: [2, 2], rightSize: [3, 2] });
    expect(summary.sizeMismatch).toBe(true);
    expect(summary.identical).toBe(false);
    expect(summary.overlapText).toBe("2×2 overlap");

    const same = formatDiffSummary({ changed: 0, compared: 4, leftSize: [2, 2], rightSize: [2, 2] });
    expect(same.identical).toBe(true);
    expect(same.overlapText).toBeNull();
  });
});
