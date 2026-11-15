import { describe, expect, it } from "vitest";
import { maxOf } from "./maxOf";

describe("maxOf", () => {
  it("is the largest value, or the floor", () => {
    expect(maxOf([3, 9, 4], (n) => n)).toBe(9);
    expect(maxOf([3, 9, 4], (n) => n, 20)).toBe(20);
    expect(maxOf([] as number[], (n) => n, 1)).toBe(1);
    expect(maxOf(["a", "abcd", ""], (s) => s.length, 2)).toBe(4);
  });
  it("takes more items than a function call takes arguments", () => {
    const many = Array.from({ length: 400_000 }, (_, i) => i % 1000);
    expect(maxOf(many, (n) => n, 4)).toBe(999);
  });
});
