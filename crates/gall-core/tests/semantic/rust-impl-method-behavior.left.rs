struct Counter { n: u32 }
impl Counter {
    fn inc(&mut self) { self.n += 1; }
    fn get(&self) -> u32 { self.n }
}
