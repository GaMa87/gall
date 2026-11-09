//! Bounded local diagnostic context. Never record IPC arguments or file contents.
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;

/// Remove free-form values which can contain local paths or credentials.
pub fn sanitize(text: &str) -> String {
    text.split_whitespace()
        .take(500)
        .map(|word| {
            let lower = word.to_ascii_lowercase();
            if word.contains('/')
                || word.contains('\\')
                || lower.contains("token")
                || lower.contains("password")
                || lower.contains("secret")
                || word.contains('=')
                || word.contains('@')
            {
                "[redacted]".to_string()
            } else {
                word.chars().take(160).collect()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Session {
    pub unclean: bool,
    #[serde(default)]
    pub identity: std::collections::BTreeMap<String, String>,
    pub events: VecDeque<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl Session {
    pub fn latest_error(&self) -> Option<&str> {
        self.last_error.as_deref().or_else(|| {
            self.events
                .iter()
                .rev()
                .find(|event| {
                    event.starts_with("error:")
                        || event.starts_with("panic:")
                        || (event.starts_with("command:")
                            && event.split_whitespace().any(|word| word == "failed"))
                })
                .map(String::as_str)
        })
    }
}

pub struct Diagnostics {
    path: PathBuf,
    pub current: Session,
    pub previous: Session,
}
impl Diagnostics {
    pub fn open(path: PathBuf) -> Self {
        Self::open_with_identity(path, Default::default())
    }
    pub fn open_with_identity(
        path: PathBuf,
        identity: std::collections::BTreeMap<String, String>,
    ) -> Self {
        let previous = std::fs::read(&path)
            .ok()
            .filter(|v| v.len() <= 262144)
            .and_then(|v| serde_json::from_slice(&v).ok())
            .unwrap_or_default();
        let this = Self {
            path,
            previous,
            current: Session {
                unclean: true,
                identity,
                ..Default::default()
            },
        };
        this.persist();
        this
    }
    pub fn record(&mut self, kind: &str, text: &str) {
        let line = format!("{}: {}", sanitize(kind), sanitize(text));
        if matches!(kind, "error" | "panic") {
            self.current.last_error = Some(line.clone());
        }
        self.current.events.push_back(line);
        while self.current.events.len() > 200
            || self.current.events.iter().map(String::len).sum::<usize>() > 240000
        {
            self.current.events.pop_front();
        }
        self.persist();
    }
    pub fn clean(&mut self) {
        self.current.unclean = false;
        self.persist();
    }
    fn persist(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(data) = serde_json::to_vec(&self.current) {
            let tmp = self.path.with_extension("tmp");
            if std::fs::write(&tmp, data).is_ok() {
                let _ = std::fs::rename(tmp, &self.path);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn previous_identity_survives_upgrade() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("diagnostics.json");
        let old = std::collections::BTreeMap::from([("version".into(), "old".into())]);
        Diagnostics::open_with_identity(path.clone(), old);
        let current = std::collections::BTreeMap::from([("version".into(), "new".into())]);
        let d = Diagnostics::open_with_identity(path, current);
        assert_eq!(d.previous.identity["version"], "old");
        assert_eq!(d.current.identity["version"], "new");
    }
    #[test]
    fn private_values_are_removed() {
        let text = sanitize("failed /home/alice/private C:\\Users\\Alice \\\\server\\share token=abc password=xyz alice@example.com");
        assert!(!text.contains("alice"));
        assert!(!text.contains("Alice"));
        assert!(!text.contains("abc"));
    }
    #[test]
    fn bounded_and_recovers_unclean_session() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("diagnostics.json");
        let mut d = Diagnostics::open(p.clone());
        for _ in 0..250 {
            d.record("command", "list_dir success");
        }
        assert_eq!(d.current.events.len(), 200);
        let mut next = Diagnostics::open(p.clone());
        assert!(next.previous.unclean);
        next.clean();
        assert!(!Diagnostics::open(p).previous.unclean);
    }
    #[test]
    fn latest_error_survives_successful_actions_and_restart() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("diagnostics.json");
        let mut d = Diagnostics::open(path.clone());
        d.record("error", "old failure");
        d.record(
            "error",
            "list_dir failed: File not found /home/alice/private",
        );
        for _ in 0..250 {
            d.record("command", "list_dir ok");
        }
        let error = d.current.latest_error().unwrap();
        assert_eq!(error, "error: list_dir failed: File not found [redacted]");
        assert!(!d
            .current
            .events
            .iter()
            .any(|event| event.starts_with("error:")));
        assert_eq!(Diagnostics::open(path).previous.latest_error(), Some(error));
    }
    #[test]
    fn legacy_session_uses_last_failure_not_last_success_or_warning() {
        let session: Session = serde_json::from_str(
            r#"{"unclean":true,"events":["error: older failure","command: 1ms list_dir failed","warn: warning","command: 2ms list_dir ok"]}"#,
        )
        .unwrap();
        assert_eq!(session.latest_error(), Some("command: 1ms list_dir failed"));
        assert!(Session::default().latest_error().is_none());
    }
}
