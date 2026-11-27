struct Counter { n: u32 }
impl Counter {
    fn add(&mut self, x: u32) { self.n += x; }
}
