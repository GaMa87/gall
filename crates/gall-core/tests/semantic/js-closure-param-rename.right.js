function mapBoth(xs, ys) {
  return [xs.map(n => n + 1), ys.map((v, idx) => v + idx)];
}
