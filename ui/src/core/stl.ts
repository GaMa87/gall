// STL (stereolithography) parsing for the model preview: binary and ASCII files into a
// triangle soup with flat normals, plus the bounding box.

export interface StlMesh {
  /** x, y, z of every triangle corner, 9 floats per triangle. */
  positions: Float32Array;
  /** One unit normal per corner (the triangle's), same length as `positions`. */
  normals: Float32Array;
  triangles: number;
  min: [number, number, number];
  max: [number, number, number];
}

const HEADER = 80;
const TRIANGLE = 50; // 12 floats + 2 attribute bytes

/** A binary STL states its triangle count; the file size must match it exactly. Some
 *  ASCII files begin with "solid" too, so the size check decides, not the word. */
function isBinary(view: DataView): boolean {
  if (view.byteLength < HEADER + 4) return false;
  const n = view.getUint32(HEADER, true);
  return HEADER + 4 + n * TRIANGLE === view.byteLength;
}

export function parseStl(buffer: ArrayBuffer): StlMesh {
  const view = new DataView(buffer);
  const positions = isBinary(view) ? readBinary(view) : readAscii(new TextDecoder().decode(buffer));
  if (positions.length === 0) throw new Error("no triangles found");
  return finish(positions);
}

function readBinary(view: DataView): Float32Array {
  const n = view.getUint32(HEADER, true);
  const out = new Float32Array(n * 9);
  for (let i = 0; i < n; i++) {
    const at = HEADER + 4 + i * TRIANGLE + 12; // skip the stored normal, recomputed below
    for (let k = 0; k < 9; k++) out[i * 9 + k] = view.getFloat32(at + k * 4, true);
  }
  return out;
}

function readAscii(text: string): Float32Array {
  const out: number[] = [];
  const re = /vertex\s+(\S+)\s+(\S+)\s+(\S+)/gi;
  for (let m = re.exec(text); m; m = re.exec(text)) out.push(Number(m[1]), Number(m[2]), Number(m[3]));
  // Whole triangles only; a truncated file keeps what is complete.
  return Float32Array.from(out.slice(0, out.length - (out.length % 9)).map((v) => (Number.isFinite(v) ? v : 0)));
}

function finish(positions: Float32Array): StlMesh {
  const triangles = positions.length / 9;
  const normals = new Float32Array(positions.length);
  const min: [number, number, number] = [Infinity, Infinity, Infinity];
  const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];
  for (let t = 0; t < triangles; t++) {
    const o = t * 9;
    for (let c = 0; c < 3; c++) {
      for (let a = 0; a < 3; a++) {
        const v = positions[o + c * 3 + a];
        if (v < min[a]) min[a] = v;
        if (v > max[a]) max[a] = v;
      }
    }
    const ux = positions[o + 3] - positions[o], uy = positions[o + 4] - positions[o + 1], uz = positions[o + 5] - positions[o + 2];
    const vx = positions[o + 6] - positions[o], vy = positions[o + 7] - positions[o + 1], vz = positions[o + 8] - positions[o + 2];
    let nx = uy * vz - uz * vy, ny = uz * vx - ux * vz, nz = ux * vy - uy * vx;
    const len = Math.hypot(nx, ny, nz) || 1; // a degenerate triangle keeps a zero normal
    nx /= len; ny /= len; nz /= len;
    for (let c = 0; c < 3; c++) {
      normals[o + c * 3] = nx;
      normals[o + c * 3 + 1] = ny;
      normals[o + c * 3 + 2] = nz;
    }
  }
  return { positions, normals, triangles, min, max };
}
