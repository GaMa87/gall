// Geometry of the filled folder icon in whole device pixels.
//
// The folder is designed on a 16-unit grid. Drawn as a 16-unit viewBox scaled to the icon's size, every
// edge lands on a fraction of a device pixel at 125 %, 150 % and so on (1.5 device px per unit at 150 %:
// an edge at x = 1 is at 1.5 px) and the icon is blurred. Here the viewBox is the icon's size in device
// pixels, 1:1, and every straight edge is rounded to a whole device pixel.
//
// The emblems of special folders (Catppuccin art) have strokes centred on half units, which only
// land on whole device pixels when a unit is a whole number of device pixels: they are drawn at an
// integer scale and placed by whole-pixel offsets.

export interface FolderGeometry {
  /** Side of the icon in device pixels (the viewBox is `0 0 size size`). */
  size: number;
  back: string;
  front: string;
  /** The light line under the folder's tab. */
  line: { y: number; x1: number; x2: number };
  /** Right and bottom edge of the folder body. */
  body: { left: number; right: number; top: number; bottom: number };
  emblem: {
    /** Device pixels per emblem unit (a whole number). */
    scale: number;
    /** SVG transform that puts the emblem's 16-unit space on the grid. */
    transform: string;
    /** Stroke width of the ring around the emblem, in emblem units: the strokes plus a whole-pixel ring on each side. */
    halo: number;
    /** Where the emblem's far corner (unit 16, 16) lands, in device pixels. */
    right: number;
    bottom: number;
  };
}

/**
 * Device pixels per emblem unit for an icon of `size` device pixels: the emblem (8 of the 16 units)
 * stays about half of the icon, as in the Catppuccin art (a third at 125 %, two thirds at 150 %,
 * where only whole numbers keep its strokes sharp).
 */
export const emblemScale = (size: number): number => Math.max(1, Math.round(size / 16));

/** Width in device pixels of the ring of the pane colour around the emblem strokes, on each side. */
export const emblemRing = (scale: number): number => Math.max(1, Math.round(scale * 0.6));

/** Folder geometry for an icon of `size` device pixels. */
export function folderGeometry(size: number): FolderGeometry {
  const n = Math.max(8, Math.round(size));
  const u = n / 16;
  const r = (v: number) => Math.round(v * u);
  // Outer edges: outwards, so the folder keeps the width the 16-unit art has at this size.
  const left = Math.floor(1 * u);
  const right = Math.ceil(15 * u);
  const top = Math.floor(2 * u);
  const bottom = Math.ceil(14 * u);
  const rad = 1.5 * u;
  const shoulder = r(3.5);
  const frontTop = r(5);
  const sideTop = r(12.5);
  const x = (v: number) => r(v);
  const a = (rr: number, px: number, py: number) => `A${rr} ${rr} 0 0 1 ${px} ${py}`;
  const back = [
    `M${left} ${shoulder}`, a(rad, x(2.5), top),
    `H${x(5.9)}`, a(rad, x(6.96), r(2.44)),
    `L${x(8.03)} ${shoulder}`, `H${x(13.5)}`, a(rad, right, frontTop),
    `V${sideTop}`, a(rad, x(13.5), bottom),
    `H${x(2.5)}`, a(rad, left, sideTop), "Z",
  ].join("");
  const rf = 1.4 * u;
  const front = [
    `M${left} ${r(6.4)}`, a(rf, x(2.4), frontTop),
    `H${x(13.6)}`, a(rf, right, r(6.4)),
    `V${sideTop}`, a(rad, x(13.5), bottom),
    `H${x(2.5)}`, a(rad, left, sideTop), "Z",
  ].join("");
  const k = emblemScale(n);
  // The emblem's lower-right corner meets the folder body's right edge and the icon's bottom edge, so
  // it never leaves the icon's box and never reaches past the folder on the right.
  const tx = right - 16 * k;
  const ty = n - 16 * k;
  return {
    size: n,
    back,
    front,
    line: { y: Math.floor(5.5 * u) + 0.5, x1: x(2), x2: x(14) },
    body: { left, right, top, bottom },
    emblem: {
      scale: k,
      transform: `translate(${tx} ${ty}) scale(${k})`,
      halo: (k + 2 * emblemRing(k)) / k,
      right,
      bottom: n,
    },
  };
}
