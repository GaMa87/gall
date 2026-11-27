import { describe, expect, it } from "vitest";
import type { MergeChunkInfo, MergeModel, MergeProposal } from "../api/types";
import type { Card } from "./mergeCards";
import { bare, canAccept, composeWithProposal, conflictInput, newLines, proposalMessage, provenance } from "./mergeProposal";

const plain = (key: string, params: Record<string, string | number> = {}) => key.replace(/\{(\w+)\}/g, (_, name) => String(params[name] ?? ""));
const span = (start: number, len: number) => ({ start, len });
const chunk = (id: string, kind: MergeChunkInfo["kind"], base: string[], left: string[], right: string[], at = 0): MergeChunkInfo => ({
  id, kind, base: span(at, base.length), left: span(at, left.length), right: span(at, right.length), baseLines: base, leftLines: left, rightLines: right, combinedLines: null,
});
// a stable line, a conflict c1, a stable line, a conflict c2, a stable line
const base = ["top\n", "b1\n", "mid\n", "b2\n", "end\n"];
const model: MergeModel = {
  chunks: [
    chunk("s0", "stable", ["top\n"], ["top\n"], ["top\n"], 0),
    chunk("c1", "conflict", ["b1\n"], ["L1\n"], ["R1\n"], 1),
    chunk("s1", "stable", ["mid\n"], ["mid\n"], ["mid\n"], 2),
    chunk("c2", "conflict", ["b2\n"], ["L2\n"], ["R2\n"], 3),
    chunk("s2", "stable", ["end\n"], ["end\n"], ["end\n"], 4),
  ],
  conflicts: ["c1", "c2"],
};

describe("conflict input", () => {
  it("sends bare lines of the three versions and a little context", () => {
    const side = (lines: string[], before: string[], after: string[]) => ({ span: span(0, lines.length), lines, kind: "changed" as const, before, after });
    const card: Card = { id: "c1", number: 1, base: side(["b\r\n"], [], []), left: side(["L\r\n"], ["a\r\n", "b\r\n", "c\r\n", "d\r\n"], ["x\r\n"]), right: side(["R\r\n"], [], []) };
    const input = conflictInput(card);
    expect(input).toEqual({ id: "c1", baseLines: ["b"], leftLines: ["L"], rightLines: ["R"], before: ["b", "c", "d"], after: ["x"] });
    expect(bare("x\n")).toBe("x");
    expect(bare("x")).toBe("x");
  });
});

describe("the result with a proposal", () => {
  it("replaces only that conflict and keeps the choices for the others", () => {
    const r = composeWithProposal(model, base, { c2: "right" }, "c1", ["both A", "both B"]);
    expect(r.text).toBe("top\nboth A\nboth B\nmid\nR2\nend\n");
    expect(r.othersOpen).toBe(false);
    expect(r.before).toBe("top\n");
    expect(r.after).toBe("mid\nR2\nend\n");
  });
  it("leaves another conflict without a choice between markers and says so", () => {
    const r = composeWithProposal(model, base, {}, "c1", ["x"]);
    expect(r.othersOpen).toBe(true);
    expect(r.text).toContain("<<<<<<< ours\nL2\n=======\nR2\n>>>>>>> theirs\n");
    expect(r.text.startsWith("top\nx\nmid\n")).toBe(true);
  });
  it("keeps CRLF files in CRLF and takes an empty proposal as a deletion", () => {
    const crlf: MergeModel = { chunks: [chunk("s0", "stable", ["a\r\n"], ["a\r\n"], ["a\r\n"], 0), chunk("c1", "conflict", ["b\r\n"], ["L\r\n"], ["R\r\n"], 1)], conflicts: ["c1"] };
    expect(composeWithProposal(crlf, ["a\r\n", "b\r\n"], {}, "c1", ["one", "two"]).text).toBe("a\r\none\r\ntwo\r\n");
    expect(composeWithProposal(crlf, ["a\r\n", "b\r\n"], {}, "c1", []).text).toBe("a\r\n");
  });
  it("computes exact before and after for syntax checks when files lack a final newline", () => {
    // Conflict before a final line that has no trailing newline, and preceding chunk without newline.
    const noEof: MergeModel = {
      chunks: [
        chunk("s0", "stable", ["fn main() {"], ["fn main() {"], ["fn main() {"], 0),
        chunk("c1", "conflict", ["let x = 1;"], ["let x = 2;"], ["let y = 3;"], 1),
        chunk("s1", "stable", ["}"], ["}"], ["}"], 2),
      ],
      conflicts: ["c1"],
    };
    const syntaxParts = composeWithProposal(noEof, ["fn main() {", "let x = 1;", "}"], {}, "c1", []);
    expect(syntaxParts.before).toBe("fn main() {\n");
    expect(syntaxParts.after).toBe("}");
    expect(syntaxParts.othersOpen).toBe(false);
    const composed = composeWithProposal(noEof, ["fn main() {", "let x = 1;", "}"], {}, "c1", ["let x = 2;", "let y = 3;"]);
    expect(composed.before).toBe("fn main() {\n");
    expect(composed.after).toBe("}");
    expect(composed.text).toBe("fn main() {\nlet x = 2;\nlet y = 3;\n}");
  });
});

describe("provenance and messages", () => {
  it("tells where each line comes from", () => {
    expect(provenance(["a", "b", "c", "d"], ["a", "b"], ["b", "c"])).toEqual(["left", "both", "right", "new"]);
    // A line only the base had (both sides changed or dropped it) is the base's, not new.
    expect(provenance(["old", "z"], ["a"], ["b"], ["old"])).toEqual(["base", "new"]);
    expect(newLines(["old", "z"], ["a"], ["b"], ["old"])).toBe(1);
    expect(newLines(["a", "z"], ["a"], [])).toBe(1);
  });
  it("has words for every end except a proposal", () => {
    const proposal: MergeProposal = { protocol: 1, revision: "r", replacement: [], explanation: "", risk: "", uncertainty: "", confidence: 0.5, evidence: [] };
    expect(proposalMessage({ outcome: "proposed", proposal, model: "m", cached: false }, plain)).toBeNull();
    for (const o of [{ outcome: "stale" }, { outcome: "disabled" }, { outcome: "refused", reason: "x" }, { outcome: "tooLarge" }, { outcome: "timeout" }, { outcome: "cancelled" }, { outcome: "failed", message: "boom" }, { outcome: "malformed", reason: "marker" }] as const) {
      expect(proposalMessage(o, plain)).toBeTruthy();
    }
    expect(proposalMessage({ outcome: "malformed", reason: "marker" }, plain)).toContain("as it was");
    // The words go through the translator with their parameters, never glued together in English.
    const seen: string[] = [];
    proposalMessage({ outcome: "failed", message: "boom" }, (k, p) => { seen.push(`${k}|${JSON.stringify(p)}`); return k; });
    expect(seen).toEqual(['The local model failed: {message}|{"message":"boom"}']);
  });
  it("can be accepted only for the revision on screen", () => {
    const p = { protocol: 1, revision: "abc", replacement: [], explanation: "", risk: "", uncertainty: "", confidence: 0, evidence: [] } as MergeProposal;
    expect(canAccept(p, "abc")).toBe(true);
    expect(canAccept(p, "abd")).toBe(false);
    expect(canAccept(p, null)).toBe(false);
  });
});
