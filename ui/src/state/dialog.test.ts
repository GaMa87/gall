import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => Promise.resolve(null)), convertFileSrc: (p: string) => p }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import * as A from "./actions";
import { getState, setState, type Overlay } from "./store";

const answer = (button: string | null) => {
  const o = getState().overlay;
  if (o?.type !== "dialog") throw new Error(`no dialog open (overlay: ${o?.type})`);
  o.dialog.resolve(button === null ? null : { button, value: "" });
};

describe("a dialog asked from inside another window", () => {
  it("returns to that window when answered (Cancel on “Discard unsaved changes?” keeps the compare view)", async () => {
    const diff: Overlay = { type: "diff", a: { kind: "file", path: "/a" }, b: { kind: "file", path: "/b" } };
    setState({ overlay: diff });
    const p = A.confirm("Discard unsaved changes?", "…", "Discard");
    expect(getState().overlay?.type).toBe("dialog");
    answer("cancel");
    expect(await p).toBe(false);
    expect(getState().overlay).toBe(diff);
  });

  it("keeps the window under the dialog while it is open", () => {
    const trash: Overlay = { type: "trash" };
    setState({ overlay: trash });
    void A.confirm("Empty the trash?", "…");
    const o = getState().overlay;
    expect(o?.type === "dialog" && o.under).toBe(trash);
    answer(null);
    expect(getState().overlay).toBe(trash);
  });

  it("closes to nothing when there was no window", async () => {
    setState({ overlay: null });
    const p = A.confirm("Delete?", "…");
    answer("ok");
    expect(await p).toBe(true);
    expect(getState().overlay).toBeNull();
  });

  it("a second dialog cancels the first instead of leaving it waiting forever", async () => {
    setState({ overlay: { type: "settings" } });
    const first = A.confirm("First?", "…");
    const second = A.confirm("Second?", "…");
    expect(await first).toBe(false);
    answer("ok");
    expect(await second).toBe(true);
    expect(getState().overlay?.type).toBe("settings");
  });
});
