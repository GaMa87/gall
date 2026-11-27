import { afterEach, describe, expect, it, vi } from "vitest";
import { dropdownBox, eventAnchor, placeSurface, submenuAnchor } from "./anchor";

const win = { w: 1000, h: 700 };
const place = (anchor: Parameters<typeof placeSurface>[0], w = 200, h = 300) => placeSurface(anchor, w, h, win.w, win.h);

describe("placeSurface", () => {
  it("puts a mouse menu at the click point", () => {
    expect(place({ x: 100, y: 120 })).toEqual({ x: 100, y: 120 });
  });

  it("keeps a mouse menu inside the window", () => {
    expect(place({ x: 990, y: 690 })).toEqual({ x: 796, y: 396 });
    expect(place({ x: -20, y: -5 })).toEqual({ x: 4, y: 4 });
  });

  it("opens beside the element, level with it, when there is room on the right", () => {
    const avoid = { left: 300, top: 200, right: 500, bottom: 224 };
    expect(place({ x: 508, y: 200, avoid })).toEqual({ x: 508, y: 200 });
  });

  it("moves a menu taller than the room below the row up, keeping it right of the text and level with the row", () => {
    // WN-67: a 441 px commit menu in a 675 px window cannot start at the row (261); it is clamped to 230 and still spans the row.
    const avoid = { left: 203, top: 261, right: 550, bottom: 289 };
    const at = placeSurface({ x: 556, y: 261, avoid }, 429, 441, 990, 675);
    expect(at).toEqual({ x: 556, y: 230 });
    expect(at.y).toBeLessThanOrEqual(avoid.bottom);
    expect(at.y + 441).toBeGreaterThanOrEqual(avoid.top);
    expect(at.x).toBeGreaterThanOrEqual(avoid.right);
  });

  it("goes below the element when the right side is too narrow, never over it", () => {
    const avoid = { left: 20, top: 200, right: 900, bottom: 224 };
    const at = place({ x: 908, y: 200, avoid });
    expect(at.y).toBe(230);
    expect(at.x + 200).toBeLessThanOrEqual(996);
  });

  it("goes above the element at the bottom of the window", () => {
    const avoid = { left: 20, top: 660, right: 900, bottom: 684 };
    const at = place({ x: 908, y: 660, avoid });
    expect(at.y + 300).toBeLessThanOrEqual(660 - 6 + 0.001);
  });

  it("falls back to the left when neither right nor below fits", () => {
    const avoid = { left: 400, top: 560, right: 900, bottom: 584 };
    expect(place({ x: 906, y: 560, avoid })).toEqual({ x: 194, y: 396 });
  });

  it("drops a button's menu below it, and above it at the bottom of the window", () => {
    const top = { left: 600, top: 40, right: 660, bottom: 64 };
    expect(place({ x: 600, y: 64, avoid: top, side: "below" })).toEqual({ x: 600, y: 70 });
    const bottom = { left: 600, top: 640, right: 660, bottom: 664 };
    expect(place({ x: 600, y: 664, avoid: bottom, side: "below" })).toEqual({ x: 600, y: 334 });
    const edge = { left: 960, top: 40, right: 990, bottom: 64 };
    expect(place({ x: 960, y: 64, avoid: edge, side: "below" })).toEqual({ x: 796, y: 70 });
  });

  it("opens a submenu right of its item, and left of the parent menu at the right edge", () => {
    expect(place({ x: 498, y: 196, avoid: { left: 300, top: 200, right: 500, bottom: 224 }, side: "submenu" })).toEqual({ x: 498, y: 196 });
    expect(place({ x: 948, y: 196, avoid: { left: 750, top: 200, right: 950, bottom: 224 }, side: "submenu" })).toEqual({ x: 552, y: 196 });
  });
});

describe("eventAnchor", () => {
  class FakeElement {
    constructor(private box: { left: number; top: number; right: number; bottom: number }) {}
    getBoundingClientRect() { return { ...this.box, width: this.box.right - this.box.left, height: this.box.bottom - this.box.top }; }
  }
  const button = () => new FakeElement({ left: 600, top: 40, right: 660, bottom: 64 });
  afterEach(() => { vi.unstubAllGlobals(); });

  it("uses the pointer of a real click and of a right click", () => {
    vi.stubGlobal("Element", FakeElement);
    expect(eventAnchor({ type: "click", detail: 1, clientX: 630, clientY: 50, currentTarget: button() })).toEqual({ x: 630, y: 50 });
    expect(eventAnchor({ type: "contextmenu", detail: 0, clientX: 630, clientY: 50, currentTarget: button() })).toEqual({ x: 630, y: 50 });
  });

  it("uses the button, not the window corner, for a click made with Enter or Space", () => {
    vi.stubGlobal("Element", FakeElement);
    const avoid = { left: 600, top: 40, right: 660, bottom: 64 };
    expect(eventAnchor({ type: "click", detail: 0, clientX: 0, clientY: 0, currentTarget: button() })).toEqual({ x: 600, y: 64, avoid, side: "below" });
    // Some engines report a position (the middle of the element) for a keyboard click.
    expect(eventAnchor({ type: "click", detail: 0, clientX: 630, clientY: 52, currentTarget: button() })).toEqual({ x: 600, y: 64, avoid, side: "below" });
  });

  it("uses the element for a context menu key without a position", () => {
    vi.stubGlobal("Element", FakeElement);
    expect(eventAnchor({ type: "contextmenu", detail: 0, clientX: 0, clientY: 0, currentTarget: null, target: button() }).avoid).toEqual({ left: 600, top: 40, right: 660, bottom: 64 });
  });

  it("anchors a submenu to its item", () => {
    expect(submenuAnchor(button() as unknown as Element)).toEqual({ x: 658, y: 36, avoid: { left: 600, top: 40, right: 660, bottom: 64 }, side: "submenu" });
  });
});

describe("dropdownBox", () => {
  const options = { minWidth: 160, maxHeight: 320 };
  it("opens below the control with its width", () => {
    expect(dropdownBox({ left: 100, top: 100, right: 400, bottom: 130 }, options, win.w, win.h)).toEqual({ left: 100, top: 135, width: 300, maxHeight: 320 });
  });
  it("opens upwards near the bottom of the window", () => {
    expect(dropdownBox({ left: 100, top: 600, right: 400, bottom: 630 }, options, win.w, win.h)).toEqual({ left: 100, bottom: 105, width: 300, maxHeight: 320 });
  });
  it("stays inside the window at the right edge and in a narrow window", () => {
    expect(dropdownBox({ left: 950, top: 100, right: 990, bottom: 130 }, options, win.w, win.h).left).toBe(832);
    expect(dropdownBox({ left: 10, top: 100, right: 400, bottom: 130 }, options, 200, win.h)).toMatchObject({ left: 8, width: 184 });
  });
});
