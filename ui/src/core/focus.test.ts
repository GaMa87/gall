import { describe, expect, it } from "vitest";
import { focusAfterRemoval } from "./focus";

const has = (list: string[]) => (p: string) => list.includes(p);

describe("cursor after the focused item disappears (trash, move, rename elsewhere)", () => {
  it("goes to the item that took its place", () => {
    expect(focusAfterRemoval(["a", "b", "c", "d"], "b", has(["a", "c", "d"]))).toBe("c");
  });
  it("skips neighbours that went too (several deleted)", () => {
    expect(focusAfterRemoval(["a", "b", "c", "d"], "b", has(["a", "d"]))).toBe("d");
  });
  it("the last item gone: the one before it", () => {
    expect(focusAfterRemoval(["a", "b", "c"], "c", has(["a", "b"]))).toBe("b");
  });
  it("nothing left: no cursor", () => {
    expect(focusAfterRemoval(["a"], "a", has([]))).toBeNull();
  });
  it("not in the old list: no guess", () => {
    expect(focusAfterRemoval(["a", "b"], "x", has(["a"]))).toBeNull();
  });
});
