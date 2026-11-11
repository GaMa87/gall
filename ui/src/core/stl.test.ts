import { describe, expect, it } from "vitest";
import { parseStl } from "./stl";

/** A binary STL with the given triangles (9 numbers each). */
function binary(tris: number[][]): ArrayBuffer {
  const buf = new ArrayBuffer(84 + tris.length * 50);
  const v = new DataView(buf);
  v.setUint32(80, tris.length, true);
  tris.forEach((t, i) => t.forEach((x, k) => v.setFloat32(84 + i * 50 + 12 + k * 4, x, true)));
  return buf;
}

const TRI = [0, 0, 0, 1, 0, 0, 0, 1, 0]; // counter-clockwise in the XY plane: normal +Z

describe("STL parsing", () => {
  it("reads a binary file and recomputes the normals", () => {
    const m = parseStl(binary([TRI, [0, 0, 2, 1, 0, 2, 0, 1, 2]]));
    expect(m.triangles).toBe(2);
    expect(Array.from(m.normals.slice(0, 3))).toEqual([0, 0, 1]);
    expect(m.min).toEqual([0, 0, 0]);
    expect(m.max).toEqual([1, 1, 2]);
  });

  it("reads an ASCII file, also with a binary-looking 'solid' header", () => {
    const text = "solid cube\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 2 0 0\n   vertex 0 2 0\n  endloop\n endfacet\nendsolid cube\n";
    const m = parseStl(new TextEncoder().encode(text).buffer);
    expect(m.triangles).toBe(1);
    expect(m.max).toEqual([2, 2, 0]);
  });

  it("reads exponent notation and keeps only complete triangles", () => {
    const text = "solid x\nvertex 1e0 0 0\nvertex 0 1E+0 0\nvertex 0 0 -1.5e0\nvertex 9 9 9\n";
    const m = parseStl(new TextEncoder().encode(text).buffer);
    expect(m.triangles).toBe(1);
    expect(m.min[2]).toBe(-1.5);
  });

  it("does not treat a mismatching size as binary", () => {
    const buf = binary([TRI]);
    new DataView(buf).setUint32(80, 5, true); // claims 5 triangles, holds 1
    expect(() => parseStl(buf)).toThrow("no triangles");
  });

  it("gives degenerate triangles a zero normal instead of NaN", () => {
    const m = parseStl(binary([[1, 1, 1, 1, 1, 1, 1, 1, 1]]));
    expect(Array.from(m.normals.slice(0, 3))).toEqual([0, 0, 0]);
  });

  it("rejects empty and unrelated input", () => {
    expect(() => parseStl(new ArrayBuffer(0))).toThrow("no triangles");
    expect(() => parseStl(new TextEncoder().encode("hello").buffer)).toThrow("no triangles");
  });
});
