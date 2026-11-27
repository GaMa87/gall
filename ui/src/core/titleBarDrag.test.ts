import { describe, expect, it, vi } from "vitest";
import { createTitleBarGestures } from "./titleBarDrag";

describe("custom title-bar gestures", () => {
  function setup() {
    const startDragging = vi.fn(async () => {});
    const toggleMaximize = vi.fn(async () => {});
    const gestures = createTitleBarGestures({ startDragging, toggleMaximize });
    return { gestures, startDragging, toggleMaximize };
  }

  it("leaves a click alone and starts dragging only after the pointer crosses the threshold", () => {
    const { gestures, startDragging } = setup();
    gestures.down({ x: 10, y: 10, button: 0 }, false);
    gestures.move({ x: 13, y: 10, buttons: 1 });
    gestures.up();
    expect(startDragging).not.toHaveBeenCalled();

    gestures.down({ x: 10, y: 10, button: 0 }, false);
    gestures.move({ x: 16, y: 10, buttons: 1 });
    gestures.move({ x: 30, y: 10, buttons: 1 });
    expect(startDragging).toHaveBeenCalledOnce();
    gestures.up();
  });

  it("keeps title-bar buttons clickable and limits double-click maximize to the drag area", () => {
    const { gestures, startDragging, toggleMaximize } = setup();
    gestures.down({ x: 10, y: 10, button: 0 }, true);
    gestures.move({ x: 40, y: 10, buttons: 1 });
    gestures.up();
    expect(gestures.doubleClick(0, true)).toBe(false);
    expect(startDragging).not.toHaveBeenCalled();
    expect(toggleMaximize).not.toHaveBeenCalled();

    expect(gestures.doubleClick(0, false)).toBe(true);
    expect(toggleMaximize).toHaveBeenCalledOnce();
  });
});
