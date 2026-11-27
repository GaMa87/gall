function mapBoth(xs, ys) {
  return [xs.map(x => x + 1), ys.map((x, i) => x + i)];
}
