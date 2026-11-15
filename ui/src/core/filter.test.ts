import { describe, expect, it } from "vitest";
import { quickFilterMatches } from "./filter";

describe("quick filter and decomposed names", () => {
  it("matches an NFD name (macOS) with composed and plain queries", () => {
    const nfd = "Poročilo.txt";
    expect(quickFilterMatches(nfd, "poročilo")).toBe(true);
    expect(quickFilterMatches(nfd, "porocilo")).toBe(true);
    expect(quickFilterMatches("Poročilo.txt", "poročilo")).toBe(true);
  });
});
