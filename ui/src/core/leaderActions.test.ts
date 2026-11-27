import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ActionDef, FileEntry } from "../api/types";
import { keyActions } from "./actionsKeys";

const mocks = vi.hoisted(() => ({ actionsForSelection: vi.fn(), actionRun: vi.fn() }));
vi.mock("../api/ipc", () => ({ api: { actionsForSelection: mocks.actionsForSelection, actionRun: mocks.actionRun } }));
vi.mock("../state/store", () => ({ getState: () => ({}), setState: vi.fn() }));

import { keyedActionsFor, runKeyedAction } from "./actionsMenu";

const defs: ActionDef[] = [
  { id: "resize", label: "Resize", pluralLabel: "Resize {count} images" },
  { id: "zip", label: "Zip" },
];
const file = (path: string): FileEntry => ({ path, name: path.split("/").pop()!, isDir: false, ext: "png" }) as FileEntry;

beforeEach(() => {
  vi.clearAllMocks();
  mocks.actionsForSelection.mockResolvedValue(defs);
  mocks.actionRun.mockResolvedValue(undefined);
});

describe("keyActions", () => {
  it("numbers the actions 1–9, 0, then a–z, in order, and drops what has no key", () => {
    const many = Array.from({ length: 40 }, (_, i) => ({ id: `a${i}` }));
    const keyed = keyActions(many);
    expect(keyed).toHaveLength(36);
    expect(keyed.slice(0, 11).map((k) => k.key).join("")).toBe("1234567890a");
    expect(keyed[35]).toEqual({ key: "z", def: { id: "a35" } });
    expect(keyActions([])).toEqual([]);
  });
});

describe("keyedActionsFor / runKeyedAction", () => {
  it("lists the actions that fit the selection with the number of selected items", async () => {
    const { items, count } = await keyedActionsFor([file("/p/a.png"), file("/p/b.png")], "/p");
    expect(items.map((i) => [i.key, i.def.id])).toEqual([["1", "resize"], ["2", "zip"]]);
    expect(count).toBe(2);
    expect(mocks.actionsForSelection).toHaveBeenCalledWith([
      { path: "/p/a.png", isDir: false, ext: "png" }, { path: "/p/b.png", isDir: false, ext: "png" },
    ]);
  });

  it("targets the folder when nothing is selected", async () => {
    const { count } = await keyedActionsFor([], "/p");
    expect(count).toBe(1);
    expect(mocks.actionsForSelection).toHaveBeenCalledWith([{ path: "/p", isDir: true, ext: "" }]);
  });

  it("runs the action on the key with the selected paths and the folder", async () => {
    expect(await runKeyedAction("2", [file("/p/a.png")], "/p", vi.fn())).toBe(true);
    expect(mocks.actionRun).toHaveBeenCalledWith("zip", ["/p/a.png"], "/p");
  });

  it("does nothing, and says so, when no action has that key", async () => {
    expect(await runKeyedAction("9", [file("/p/a.png")], "/p", vi.fn())).toBe(false);
    expect(mocks.actionRun).not.toHaveBeenCalled();
  });

  it("finds nothing when the backend fails, and reports a failing action", async () => {
    mocks.actionsForSelection.mockRejectedValue(new Error("no backend"));
    expect(await runKeyedAction("1", [], "/p", vi.fn())).toBe(false);
    mocks.actionsForSelection.mockResolvedValue(defs);
    const boom = new Error("exit 1");
    mocks.actionRun.mockRejectedValue(boom);
    const onError = vi.fn();
    expect(await runKeyedAction("1", [], "/p", onError)).toBe(true);
    expect(onError).toHaveBeenCalledWith(boom);
  });
});
