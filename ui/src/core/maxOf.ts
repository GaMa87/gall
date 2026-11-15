/** The largest `value(item)`, at least `floor`. A loop, not `Math.max(...items)`: a call
 *  takes only so many arguments, and a folder of 150 000 entries is more than that
 *  (RangeError: the list would not appear at all). */
export function maxOf<T>(items: readonly T[], value: (item: T) => number, floor = -Infinity): number {
  let max = floor;
  for (const item of items) {
    const v = value(item);
    if (v > max) max = v;
  }
  return max;
}
