// Miller column widths (N-01, N-03). Pure, unit-tested.

/**
 * Column widths that always fill `avail`: few columns stretch across the whole
 * width, more columns shrink (older ones first, never below `minW`). A hovered
 * column that is narrower than `maxW` grows to `maxW` and the others give way so
 * the total stays the same; only when everything is at `minW` does the pane scroll.
 * Hovering never makes a column narrower: a column shrinking away from under the
 * pointer would fire mouseleave, un-hover, grow back and loop.
 */
export function fitColumnWidths(n: number, avail: number, minW: number, maxW: number, hovered: number | null): number[] {
  if (n === 0) return [];
  const w = baseWidths(n, avail, minW, maxW);
  if (hovered !== null && hovered < n && w[hovered] < maxW) {
    let need = maxW - w[hovered];
    w[hovered] = maxW;
    // Take the room from the other columns, older (left) ones first. Either way the
    // hovered column's edges only move outwards, so it stays under the pointer.
    for (let i = 0; i < n && need > 0; i++) {
      if (i === hovered) continue;
      const cut = Math.min(need, w[i] - minW);
      if (cut > 0) {
        w[i] -= cut;
        need -= cut;
      }
    }
  }
  // Whole pixels; give the rounding remainder to the last column so nothing overflows.
  const out = w.map((x) => Math.floor(x));
  const rest = Math.floor(avail) - out.reduce((a, x) => a + x, 0);
  if (rest > 0) out[n - 1] += rest;
  return out;
}

/** Widths without hover: preferred sizes, then stretched or compressed to exactly `avail`. */
function baseWidths(n: number, avail: number, minW: number, maxW: number): number[] {
  const even = avail / n;
  // Preferred widths: nearest columns get a comfortable width, older ones the even share.
  const w = Array.from({ length: n }, (_, i) => {
    const fromEnd = n - 1 - i;
    let x = Math.max(minW, Math.min(maxW, even));
    if (even < maxW && fromEnd < 2) x = Math.min(maxW, Math.max(x, avail * (fromEnd === 0 ? 0.35 : 0.25)));
    return x;
  });
  let diff = avail - w.reduce((a, x) => a + x, 0);
  if (diff > 0) {
    // Spare room: share it out so the columns span the full width.
    for (let i = 0; i < n; i++) w[i] += diff / n;
  } else {
    // Too wide: compress older columns first, then the newest, never below minW.
    for (let i = 0; i < n && diff < 0; i++) {
      const cut = Math.min(-diff, w[i] - minW);
      if (cut > 0) {
        w[i] -= cut;
        diff += cut;
      }
    }
  }
  return w;
}
