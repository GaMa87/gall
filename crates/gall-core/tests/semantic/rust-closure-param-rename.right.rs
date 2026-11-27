fn transform(xs: &[u32], ys: &[u32]) -> (Vec<u32>, Vec<u32>) {
    (xs.iter().map(|a| a + 1).collect(), ys.iter().map(|b| b * 2).collect())
}
