import { describe, expect, it } from "vitest";
import { quickToWildcard } from "./path";

describe("quickToWildcard", () => {
  it("keeps a plain term (substring match)", () => {
    expect(quickToWildcard("report")).toBe("report");
    expect(quickToWildcard("  *.pdf ")).toBe("*.pdf");
  });
  it("turns Quick Open syntax into a wildcard", () => {
    expect(quickToWildcard("ext:pdf")).toBe("*.pdf");
    expect(quickToWildcard("rep ext:.pdf")).toBe("*rep*.pdf");
    expect(quickToWildcard("'exact")).toBe("exact");
    expect(quickToWildcard("^start end$")).toBe("*start*end*");
    expect(quickToWildcard("main !test")).toBe("main");
    expect(quickToWildcard("src/main")).toBe("main");
    expect(quickToWildcard("src/")).toBe("src");
  });
  it("is empty when nothing is left", () => {
    expect(quickToWildcard("!x")).toBe("");
    expect(quickToWildcard("")).toBe("");
  });
});
