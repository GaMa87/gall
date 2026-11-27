// Pixel-grid helpers. A 16 px icon with 1 px strokes drawn on half-pixel coordinates (as in the
// Catppuccin icons) is only crisp when its box starts on a whole device pixel. Centering an icon
// in a row whose spare space is odd puts it on a half pixel, and at fractional display scales
// (125 %, 150 %) most positions are off the grid, so heights are chosen to keep the centred
// offsets whole.

/** Snaps a CSS length to a whole number of device pixels. */
export const snapCss = (css: number, dpr: number): number => Math.round(css * dpr) / dpr;

const isWhole = (v: number) => Math.abs(v - Math.round(v)) < 1e-6;

/**
 * Height of a list row (CSS px) of at least `base`, whose box is `border` px thick on each side,
 * so that an `icon` px square centred inside it starts on a whole device pixel at `dpr`
 * (the spare space on each side is a whole, preferably even, number of device pixels).
 */
export function alignedRowHeight(base: number, icon: number, dpr: number, border = 1): number {
  const d = Math.max(1, dpr);
  for (let step = 0; step <= 12; step++) {
    // Candidate heights in device-pixel steps so the row itself is a whole number of device px too.
    const h = Math.ceil(base * d + step) / d;
    // Browsers draw a border as a whole number of device pixels (at least one).
    const borderDev = Math.max(1, Math.floor(border * d));
    const spareDev = h * d - 2 * borderDev - icon * d; // both sides together, in device px
    if (spareDev >= 0 && isWhole(spareDev) && Math.round(spareDev) % 2 === 0 && isWhole(h * d)) return h;
  }
  return Math.round(base);
}

/**
 * The CSS length that browsers draw as exactly `n` whole device pixels at `dpr`. A border is
 * floored to device pixels (1.5 px at 150 % becomes 1 px, and 1.333 px, which should be 2 device
 * pixels, can fall to 1 because layout stores lengths in 1/64 px), so the length is the smallest
 * multiple of 1/64 CSS px that reaches `n` device pixels. Box-shadow rings are not snapped at all and
 * come out blurred unless they use such a length.
 */
export const devicePx = (n: number, dpr: number): number => Math.ceil((n / dpr) * 64 - 1e-9) / 64;

/** A 2 CSS px pane frame, rounded to whole device pixels for clean corners. */
export const paneFrameDevicePx = (dpr: number): number => Math.max(1, Math.round(2 * dpr));
