//! Bounded local diagnostic context. Never record IPC arguments or file contents.
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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
    /// Number of the latest change to `current`.
    seq: u64,
    /// `current` has ordinary events the file does not hold yet.
    dirty: bool,
    /// Serializes writers of the file; holds the `seq` of the newest state written.
    written: Arc<Mutex<u64>>,
}

/// A state of the session to write to the file without holding the [`Diagnostics`] lock.
pub struct PendingWrite {
    path: PathBuf,
    seq: u64,
    data: Vec<u8>,
    written: Arc<Mutex<u64>>,
}

impl PendingWrite {
    pub fn write(self) {
        write_state(&self.written, &self.path, self.seq, &self.data);
    }
}

/// Replaces the file with `data`, unless a newer state was written meanwhile.
fn write_state(written: &Mutex<u64>, path: &Path, seq: u64, data: &[u8]) {
    let mut newest = written.lock().unwrap_or_else(|e| e.into_inner());
    if seq < *newest {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, data).is_ok() && std::fs::rename(tmp, path).is_ok() {
        *newest = seq;
    }
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
        let mut this = Self {
            path,
            previous,
            current: Session {
                unclean: true,
                identity,
                ..Default::default()
            },
            seq: 0,
            dirty: false,
            written: Arc::default(),
        };
        this.persist();
        this
    }
    /// Sets one fact about this installation that was not known when the session began.
    pub fn set_identity(&mut self, key: &str, value: &str) {
        self.current.identity.insert(key.into(), value.into());
        self.persist();
    }
    /// Adds an event. An error or panic is written to the file at once. Ordinary events
    /// (one per command of the UI) only mark the session changed: the owner writes them
    /// with [`Self::pending`] or [`Self::flush`] every few seconds, because rewriting the
    /// file for each of them cost a file write per command on the calling thread.
    pub fn record(&mut self, kind: &str, text: &str) {
        let line = format!("{}: {}", sanitize(kind), sanitize(text));
        let urgent = matches!(kind, "error" | "panic");
        if urgent {
            self.current.last_error = Some(line.clone());
        }
        self.current.events.push_back(line);
        while self.current.events.len() > 200
            || self.current.events.iter().map(String::len).sum::<usize>() > 240000
        {
            self.current.events.pop_front();
        }
        self.seq += 1;
        if urgent {
            self.persist();
        } else {
            self.dirty = true;
        }
    }
    pub fn clean(&mut self) {
        self.current.unclean = false;
        self.seq += 1;
        self.persist();
    }
    /// Marks the session as running again after a [`Self::clean`] that did not end it
    /// (an update that failed to install).
    pub fn reopen(&mut self) {
        self.current.unclean = true;
        self.seq += 1;
        self.persist();
    }
    /// The unwritten state, if any, to write after the lock on `self` is released.
    pub fn pending(&mut self) -> Option<PendingWrite> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        Some(PendingWrite {
            path: self.path.clone(),
            seq: self.seq,
            data: serde_json::to_vec(&self.current).ok()?,
            written: self.written.clone(),
        })
    }
    /// Writes unwritten events now.
    pub fn flush(&mut self) {
        if let Some(pending) = self.pending() {
            pending.write();
        }
    }
    fn persist(&mut self) {
        self.dirty = false;
        if let Ok(data) = serde_json::to_vec(&self.current) {
            write_state(&self.written, &self.path, self.seq, &data);
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
        let mut d = d;
        d.set_identity("installation", "rpm");
        let again = Diagnostics::open(d.path.clone());
        assert_eq!(again.previous.identity["installation"], "rpm");
        assert_eq!(again.previous.identity["version"], "new");
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
        d.flush();
        let mut next = Diagnostics::open(p.clone());
        assert_eq!(next.previous.events.len(), 200);
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
        let error = d.current.latest_error().unwrap().to_owned();
        assert_eq!(error, "error: list_dir failed: File not found [redacted]");
        assert!(!d
            .current
            .events
            .iter()
            .any(|event| event.starts_with("error:")));
        d.flush();
        assert_eq!(
            Diagnostics::open(path).previous.latest_error(),
            Some(error.as_str())
        );
    }
    #[test]
    fn ordinary_events_wait_for_a_flush_and_errors_are_written_at_once() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("diagnostics.json");
        let on_disk =
            || -> Session { serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap() };
        let mut d = Diagnostics::open(path.clone());
        d.record("command", "list_dir ok");
        d.record("command", "watch ok");
        assert!(on_disk().events.is_empty());
        d.record("error", "list_dir failed");
        assert_eq!(on_disk().events.len(), 3);
        assert!(d.pending().is_none());
        d.record("command", "unwatch ok");
        assert_eq!(on_disk().events.len(), 3);
        d.flush();
        assert_eq!(on_disk().events.len(), 4);
        assert!(d.pending().is_none());
    }
    #[test]
    fn an_older_pending_state_never_replaces_a_newer_one() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("diagnostics.json");
        let mut d = Diagnostics::open(path.clone());
        d.record("command", "list_dir ok");
        let older = d.pending().unwrap();
        d.record("error", "list_dir failed");
        older.write();
        let on_disk: Session = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(on_disk.events.len(), 2);
        assert!(on_disk.last_error.is_some());
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
