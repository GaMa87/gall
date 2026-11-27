trait Store {
    fn get(&self, key: &str) -> Option<String>;
}
