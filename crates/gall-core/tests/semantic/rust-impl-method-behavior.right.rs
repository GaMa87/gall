struct Counter { n: u32 }
impl Counter {
    fn inc(&mut self) { self.n += 2; }
    fn get(&self) -> u32 { self.n }
}
