import { afterEach, describe, expect, it, vi } from "vitest";

const modifiers = vi.fn(() => Promise.resolve({ known: true, ctrl: false, shift: false, alt: false }));
vi.mock("../api/ipc", () => ({ api: { modifiers, uiLog: () => Promise.resolve() } }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

type L = (e: unknown) => void;

describe("a drag from another app that leaves the window without a drop", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  it("stops polling the modifier keys", async () => {
    vi.useFakeTimers();
    const listeners = new Map<string, L[]>();
    vi.stubGlobal("window", {
      innerWidth: 800,
      innerHeight: 600,
      addEventListener: (t: string, f: L) => listeners.set(t, [...(listeners.get(t) ?? []), f]),
      setInterval: (f: () => void, ms: number) => setInterval(f, ms),
    });
    vi.stubGlobal("requestAnimationFrame", (f: () => void) => setTimeout(f, 16));
    vi.resetModules();
    await import("./dnd");
    const fire = (t: string, e: unknown) => listeners.get(t)?.forEach((f) => f(e));

    fire("dragover", { clientX: 100, clientY: 100, dataTransfer: { types: ["Files"] } });
    await vi.advanceTimersByTimeAsync(200);
    expect(modifiers).toHaveBeenCalled();
    // The pointer leaves through the window's edge; no drop, no dragend (another app owns the drag).
    fire("dragleave", { clientX: 0, clientY: 300 });
    modifiers.mockClear();
    await vi.advanceTimersByTimeAsync(1000);
    expect(modifiers).not.toHaveBeenCalled();
  });
});
