import { describe, expect, it } from "vitest";
import { emptyHiddenText, footerHiddenText, formatHiddenCount } from "./hiddenHint";

describe("hidden-match hint formatting", () => {
  it("formats singular, plural and capped hidden counts", () => {
    expect(formatHiddenCount({ count: 1, capped: false })).toBe("1");
    expect(formatHiddenCount({ count: 100, capped: true })).toBe("100+");

    expect(emptyHiddenText({ count: 1, capped: false })).toBe("1 hidden file matches");
    expect(emptyHiddenText({ count: 4, capped: false })).toBe("4 hidden files match");
    expect(emptyHiddenText({ count: 100, capped: true })).toBe("100+ hidden files match");
    expect(emptyHiddenText({ count: 1, capped: true })).toBe("1+ hidden files match");

    expect(footerHiddenText({ count: 1, capped: false })).toBe("+1 hidden by filters");
    expect(footerHiddenText({ count: 100, capped: true })).toBe("+100+ hidden by filters");
  });
});
