import { describe, expect, it } from "vitest";
import { alignedRowHeight, devicePx, paneFrameDevicePx, snapCss } from "./pixelGrid";

describe("pixel grid", () => {
  it("snaps to device pixels", () => {
    expect(snapCss(10.3, 1)).toBe(10);
    expect(snapCss(10.3, 2)).toBe(10.5);
    expect(snapCss(10.3, 1.25)).toBeCloseTo(10.4);
  });

  it("devicePx is drawn as exactly n device pixels once the browser floors it", () => {
    for (const dpr of [1, 1.1, 1.25, 1.33, 1.5, 1.75, 2, 2.25, 2.5, 3]) {
      for (const n of [1, 2, 3]) {
        const css = devicePx(n, dpr);
        // Layout keeps 1/64 px: the stored length is a multiple of it, then borders are floored.
        expect(css * 64, `${dpr} ${n}`).toBeCloseTo(Math.round(css * 64), 6);
        expect(Math.floor(css * dpr + 1e-9), `${dpr} ${n}`).toBe(n);
      }
    }
    expect(devicePx(2, 1.5)).toBe(86 / 64);
    expect(devicePx(1, 1)).toBe(1);
  });

  it("rounds the 2 CSS px pane frame to whole device pixels", () => {
    expect([1, 1.1, 1.25, 1.5, 2, 3].map(paneFrameDevicePx)).toEqual([2, 2, 3, 3, 4, 6]);
  });

  it("keeps an icon centred on whole device pixels at common scales", () => {
    for (const dpr of [1, 1.25, 1.5, 1.75, 2, 2.5, 3]) {
      for (const [base, rawIcon] of [[25, 16], [27, 18], [24, 16], [31, 20]] as const) {
        const icon = snapCss(rawIcon, dpr); // callers snap the icon size first
        const h = alignedRowHeight(base, icon, dpr);
        const sideDev = (h * dpr - 2 * Math.max(1, Math.floor(dpr)) - icon * dpr) / 2;
        expect(h, `${dpr} ${base}`).toBeGreaterThanOrEqual(base - 1e-9);
        expect(h - base, `${dpr} ${base}`).toBeLessThan(5);
        expect(Math.abs(h * dpr - Math.round(h * dpr)), `row ${dpr} ${base}`).toBeLessThan(1e-6);
        expect(Math.abs(sideDev - Math.round(sideDev)), `side ${dpr} ${base} ${h}`).toBeLessThan(1e-6);
      }
    }
  });

  it("only adds a pixel to the usual 25 px row at 1x", () => {
    expect(alignedRowHeight(25, 16, 1)).toBe(25 + 1);
  });
});
