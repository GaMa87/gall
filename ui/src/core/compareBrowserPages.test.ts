import { describe, expect, it, vi } from "vitest";
import { browserDiffPageClose, browserDiffPageHunks, browserDiffPageOpen, browserDiffPageSmall, firstDifference, linesOf } from "./compareBrowserPages";
import type { CompareTransport } from "./compareTransport";
import type { DiffOptions } from "../api/types";

const web = { desktop: false, hasWorker: true, hasWebAssembly: true };
const opts: DiffOptions = { ignoreCase: false, ignoreEol: false, minimal: false, structural: false };
const text = (t: string) => ({ kind: "text" as const, label: "x", text: t });
const answering = (answer: unknown) => ({ compare: vi.fn().mockResolvedValue(answer), cancel: vi.fn() }) as unknown as CompareTransport & { cancel: ReturnType<typeof vi.fn> };

describe("browser page commands", () => {
  it("splits lines without terminators", () => {
    expect(linesOf("a\r\nb\n")).toEqual(["a", "b"]);
    expect(linesOf("a\n\n")).toEqual(["a", ""]);
    expect(linesOf("")).toEqual([]);
  });

  it("holds a text answer for the page and pages its hunks", async () => {
    const ops = [{ tag: "equal", a: 0, aLen: 1, b: 0, bLen: 1 }, { tag: "replace", a: 1, aLen: 1, b: 1, bLen: 1 }, { tag: "insert", a: 2, aLen: 0, b: 2, bLen: 1 }];
    const t = answering({ kind: "text", ops, moves: [], added: 2, removed: 1, identical: false });
    const summary = await browserDiffPageOpen(7, text("a\nb\n"), text("a\nc\nd\n"), opts, t, web);
    expect(summary.text).toMatchObject({ added: 2, removed: 1 });
    expect(browserDiffPageSmall(7)?.rightLines).toEqual(["a", "c", "d"]);
    const page = browserDiffPageHunks(7, 0, 1);
    expect(page).toMatchObject({ total: 2, next: 1 });
    expect(page.items[0].tag).toBe("replace");
    browserDiffPageClose(7, t);
    expect(browserDiffPageSmall(7)).toBeNull();
  });

  it("classifies binary input and says where it first differs", async () => {
    expect(firstDifference(new Uint8Array([1, 2]), new Uint8Array([1, 3]))).toBe(1);
    expect(firstDifference(new Uint8Array([1]), new Uint8Array([1]))).toBeNull();
    const summary = await browserDiffPageOpen(8, text("a\0b"), text("a\0c"), opts, answering({ kind: "binary" }), web);
    expect(summary.binary).toMatchObject({ identical: false, firstDifference: 2 });
  });

  it("turns every refusal into an error with its reason", async () => {
    await expect(browserDiffPageOpen(9, text("a"), text("b"), opts, answering({}), { ...web, hasWorker: false })).rejects.toThrow(/Web Workers/);
    await expect(browserDiffPageOpen(9, { kind: "file", path: "/x" }, text("b"), opts, answering({}), web)).rejects.toThrow(/uploaded/);
    expect((await browserDiffPageOpen(9, text("a"), text("b"), opts, answering({ kind: "tooLarge", reason: "big" }), web)).limit).toBe("tooManyBytes");
  });

  it("stops a running comparison when the window closes", async () => {
    let finish: (a: unknown) => void = () => {};
    const t = { compare: vi.fn(() => new Promise((r) => { finish = r; })), cancel: vi.fn() } as unknown as CompareTransport & { cancel: ReturnType<typeof vi.fn> };
    const open = browserDiffPageOpen(10, text("a"), text("b"), opts, t, web);
    await Promise.resolve();
    browserDiffPageClose(10, t);
    expect(t.cancel).toHaveBeenCalled();
    finish({ kind: "cancelled" });
    await expect(open).rejects.toThrow(/cancelled/);
  });
});
