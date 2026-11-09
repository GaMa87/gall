import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), convertFileSrc: (p: string) => p }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { api } from "../api/ipc";
import { calcFolderSizes, folderSizeBytes, folderSizeDetails, folderSizeLabel, folderSizeTitle } from "./actions";
import { getState, setState, type FolderSize } from "./store";
import { sortEntries } from "../core/sort";
import type { FileEntry } from "../api/types";

const virtual: FolderSize = { bytes: 0, files: 0, dirs: 0, unreadable: 0, virtualFs: true, done: true, at: 1 };
const ordinary: FolderSize = { bytes: 123, files: 1, dirs: 0, unreadable: 0, done: true, at: 1 };

beforeEach(() => {
  vi.restoreAllMocks();
  setState({ folderSizes: {} });
});

describe("virtual folder sizes", () => {
  it("shows an explicit unavailable size instead of zero bytes or counting", () => {
    expect(folderSizeLabel(virtual)).toBe("—");
    expect(folderSizeTitle(virtual)).toBe("Virtual filesystem; folder size is not applicable");
    expect(folderSizeDetails(virtual)).toBe("— Virtual filesystem; folder size is not applicable");
    expect(folderSizeDetails(virtual)).not.toContain("0 bytes");
    expect(folderSizeDetails(virtual)).not.toContain("counting");
    expect(folderSizeLabel(ordinary)).toBe("123 B");
    expect(folderSizeDetails(ordinary)).toBe("123 B (123 bytes) in 1 files, 0 folders");
  });

  it("keeps virtual folder sizes unavailable to numeric sorting", () => {
    expect(folderSizeBytes(virtual)).toBeUndefined();
    expect(folderSizeBytes({ ...ordinary, done: false })).toBeUndefined();
    expect(folderSizeBytes({ ...ordinary, bytes: 0 })).toBe(0);
    const sizes = { "/empty": { ...ordinary, bytes: 0 }, "/virtual": virtual };
    const entries = ["empty", "virtual"].map((name) => ({ name, path: "/" + name, isDir: true }) as FileEntry);
    const sorted = sortEntries(entries, { key: "size", descending: false, foldersOnTop: true, foldersByName: false },
      (p) => folderSizeBytes(sizes[p as keyof typeof sizes]));
    expect(sorted.map((entry) => entry.name)).toEqual(["virtual", "empty"]);
  });

  it("accepts the marker for manual, automatic and Properties requests", async () => {
    const count = vi.spyOn(api, "folderSize").mockResolvedValue({ bytes: 0, files: 0, dirs: 0, unreadable: 0, virtualFs: true });
    for (const [owner, freshness] of [[0, 0], [1, 120_000], ["properties", 30_000]] as const) {
      const path = `/kernel-view/${owner}`;
      await calcFolderSizes(owner, [path], freshness);
      expect(getState().folderSizes[path]).toMatchObject({ virtualFs: true, done: true });
      expect(folderSizeLabel(getState().folderSizes[path])).toBe("—");
    }
    expect(count).toHaveBeenCalledTimes(3);
    await calcFolderSizes(1, ["/kernel-view/1"], 120_000);
    expect(count).toHaveBeenCalledTimes(3);
  });
});
