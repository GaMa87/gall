import { describe, expect, it } from "vitest";
import { ownsClipboard } from "./clipOwner";

const ours = ["/home/u/a.txt", "/home/u/b c.txt"];

describe("ownsClipboard", () => {
  it("keeps Gall's files while the clipboard still lists them", () => {
    expect(ownsClipboard(ours, { kind: "paths", paths: ours })).toBe(true);
    // A copied file was deleted meanwhile: the list reads back as plain text.
    expect(ownsClipboard(ours, { kind: "text", text: "/home/u/a.txt\r\n/home/u/b c.txt\n" })).toBe(true);
    expect(ownsClipboard(ours, { kind: "empty" })).toBe(true);
  });
  it("gives way to anything copied later in another program", () => {
    expect(ownsClipboard(ours, { kind: "text", text: "some notes" })).toBe(false);
    expect(ownsClipboard(ours, { kind: "image", width: 10, height: 10 })).toBe(false);
    expect(ownsClipboard(ours, { kind: "url", url: "https://example.com" })).toBe(false);
    expect(ownsClipboard(ours, { kind: "paths", paths: ["/home/u/other.txt"] })).toBe(false);
  });
  it("owns nothing without files of its own", () => {
    expect(ownsClipboard([], { kind: "empty" })).toBe(false);
  });
});
