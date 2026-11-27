fn transform(xs: &[u32], ys: &[u32]) -> (Vec<u32>, Vec<u32>) {
    (xs.iter().map(|x| x + 1).collect(), ys.iter().map(|x| x * 2).collect())
}
