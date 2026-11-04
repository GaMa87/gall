use serde::{Deserialize, Serialize};

/// Browser-style back/forward history for one tab (N-07).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct History {
    back: Vec<String>,
    forward: Vec<String>,
}

const MAX_HISTORY: usize = 200;

impl History {
    /// Records leaving `from` for a new location; clears forward history.
    pub fn visit(&mut self, from: &str) {
        if self.back.last().map(String::as_str) != Some(from) {
            self.back.push(from.to_string());
            if self.back.len() > MAX_HISTORY {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    /// Returns the previous location, pushing `current` onto the forward stack.
    pub fn back(&mut self, current: &str) -> Option<String> {
        let p = self.back.pop()?;
        self.forward.push(current.to_string());
        Some(p)
    }

    pub fn forward(&mut self, current: &str) -> Option<String> {
        let p = self.forward.pop()?;
        self.back.push(current.to_string());
        Some(p)
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }
    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_forward_cycle() {
        let mut h = History::default();
        h.visit("/a"); // now at /b
        h.visit("/b"); // now at /c
        assert_eq!(h.back("/c").as_deref(), Some("/b"));
        assert_eq!(h.back("/b").as_deref(), Some("/a"));
        assert_eq!(h.back("/a"), None);
        assert_eq!(h.forward("/a").as_deref(), Some("/b"));
        h.visit("/b"); // navigating elsewhere clears forward
        assert!(!h.can_forward());
    }
}
