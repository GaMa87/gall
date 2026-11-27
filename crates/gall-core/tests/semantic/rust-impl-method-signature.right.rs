struct Counter { n: u32 }
impl Counter {
    fn add(&mut self, x: u64) { self.n += x as u32; }
}
