import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { colouredLine, gitignoreHighlight } from "./MergeHighlight";

describe("merge syntax highlighting", () => {
  it("colours gitignore comments and unescaped patterns, including Windows paths", () => {
    const syntax = gitignoreHighlight("C:\\project\\.gitignore", ["# comment", "!*.log", "\\*literal"]);
    expect(syntax?.lines).toEqual([[[0, 9, 1]], [[0, 1, 5], [1, 2, 5]], []]);
    expect(gitignoreHighlight("file.txt", ["# comment"])).toBeNull();
  });

  it("keeps text escaped and line endings intact with UTF-16 syntax offsets", () => {
    const syntax = { language: "test", lines: [[[2, 3, 5] as [number, number, number]]] };
    expect(renderToStaticMarkup(colouredLine("😀<\r\n", syntax, 0))).toBe('😀<span class="sx5">&lt;</span>\r\n');
  });
});
