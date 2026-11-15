import { describe, expect, it } from "vitest";
import { balanceColumns } from "./balance";

describe("balanceColumns", () => {
  it("keeps the order and uses every item once", () => {
    const weights = [5, 1, 9, 3, 3, 7, 2, 8, 4];
    for (const columns of [1, 2, 3, 4]) {
      const groups = balanceColumns(weights, columns);
      expect(groups.flat()).toEqual(weights.map((_, i) => i));
      expect(groups.length).toBeLessThanOrEqual(columns);
    }
  });

  it("splits equal items evenly", () => {
    expect(balanceColumns([1, 1, 1, 1, 1, 1], 3)).toEqual([[0, 1], [2, 3], [4, 5]]);
    expect(balanceColumns([1, 1, 1, 1], 2)).toEqual([[0, 1], [2, 3]]);
  });

  it("gives a heavy item its own column instead of overfilling the last one", () => {
    expect(balanceColumns([10, 1, 1], 3)).toEqual([[0], [1, 2]]);
    expect(balanceColumns([1, 1, 10], 3)).toEqual([[0, 1], [2]]);
  });

  it("keeps every column within one heaviest item of an equal share", () => {
    const weights = [12, 6, 9, 14, 30, 17, 11, 5, 13, 16, 15, 4, 7, 5, 3];
    const share = weights.reduce((s, w) => s + w, 0) / 3;
    const sums = balanceColumns(weights, 3).map((g) => g.reduce((s, i) => s + weights[i], 0));
    expect(sums).toHaveLength(3);
    for (const sum of sums) expect(Math.abs(sum - share)).toBeLessThanOrEqual(Math.max(...weights));
  });

  it("handles no items, one column, weightless items and a column count that is not a number", () => {
    expect(balanceColumns([2, 2], Number.NaN)).toEqual([[0, 1]]);
    expect(balanceColumns([Number.NaN, 2, 2], 2)).toEqual([[0, 1], [2]]);
    expect(balanceColumns([], 3)).toEqual([]);
    expect(balanceColumns([3, 2], 1)).toEqual([[0, 1]]);
    expect(balanceColumns([0, 0], 3)).toEqual([[0, 1]]);
    expect(balanceColumns([2, 2], 0)).toEqual([[0, 1]]);
  });
});
