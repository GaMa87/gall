import { describe, expect, it } from "vitest";
import { MAX_BROWSER_INPUT_BYTES, readInput, readPair, textInput, type ReadableFile } from "./compareInputs";

const file = (name: string, text: string, size = text.length): ReadableFile => ({
  name,
  size,
  arrayBuffer: async () => new TextEncoder().encode(text).buffer as ArrayBuffer,
});

describe("browser inputs", () => {
  it("reads a file and a file handle into bytes", async () => {
    const a = await readInput(file("a.txt", "hello"));
    expect(a).toMatchObject({ ok: true, name: "a.txt" });
    expect(a.ok && Array.from(a.bytes)).toEqual([104, 101, 108, 108, 111]);
    const b = await readInput({ getFile: async () => file("b.txt", "x") });
    expect(b).toMatchObject({ ok: true, name: "b.txt" });
  });
  it("refuses a file over the limit before reading it, and one whose bytes turn out larger", async () => {
    let read = false;
    const big: ReadableFile = { name: "big.bin", size: MAX_BROWSER_INPUT_BYTES + 1, arrayBuffer: async () => { read = true; return new ArrayBuffer(0); } };
    const refused = await readInput(big);
    expect(refused).toMatchObject({ ok: false, name: "big.bin" });
    expect(refused.ok === false && refused.reason).toContain("32 MiB");
    expect(read).toBe(false);
    // The size said is small but the bytes are not.
    const liar: ReadableFile = { name: "liar.bin", size: 1, arrayBuffer: async () => new ArrayBuffer(11) };
    expect((await readInput(liar, 10)).ok).toBe(false);
  });
  it("turns every failure into a reason", async () => {
    const opened = await readInput({ getFile: async () => { throw new Error("denied"); } });
    expect(opened).toMatchObject({ ok: false });
    expect(opened.ok === false && opened.reason).toContain("denied");
    const unreadable = await readInput({ name: "u", size: 1, arrayBuffer: async () => { throw new Error("gone"); } });
    expect(unreadable.ok === false && unreadable.reason).toContain("gone");
  });
  it("reads a pair and names the side that failed", async () => {
    const both = await readPair(file("l.txt", "a"), file("r.txt", "b"));
    expect(both.ok).toBe(true);
    const bad = await readPair(file("l.txt", "a"), { name: "r.bin", size: 100, arrayBuffer: async () => new ArrayBuffer(100) }, 10);
    expect(bad.ok === false && bad.reason).toContain("r.bin");
    expect(textInput("t", "é")).toMatchObject({ ok: true, bytes: new Uint8Array([195, 169]) });
  });
});
