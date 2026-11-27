import { expect, it } from "vitest";
import { splitLines } from "./gitMerge";

it("cuts a text into lines that keep their terminators and join back to the text", () => {
  expect(splitLines("a\nb\n")).toEqual(["a\n", "b\n"]);
  expect(splitLines("a\r\nb\r\n")).toEqual(["a\r\n", "b\r\n"]);
  expect(splitLines("a\nb")).toEqual(["a\n", "b"]);
  expect(splitLines("")).toEqual([]);
  expect(splitLines("\n\n")).toEqual(["\n", "\n"]);
  for (const text of ["", "x", "x\n", "a\r\n\r\nb", "\r", "tab\tin\nlast"]) expect(splitLines(text).join("")).toBe(text);
});
