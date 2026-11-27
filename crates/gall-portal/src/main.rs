//! `xdg-desktop-portal-gall`: the FileChooser backend of xdg-desktop-portal that shows Gall's file
//! picker (`gall --pick`) for the "Open file" and "Save as" dialogs of other programs.
//!
//! Enable it with `org.freedesktop.impl.portal.FileChooser=gall;gtk` in a portals.conf (Omarchy:
//! `hyprland-portals.conf`). The portal asks; this process starts `gall --pick …` and waits for the
//! JSON file Gall writes when the user decides.

// Only the Linux service uses it; elsewhere it is built for its tests.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod request;

#[cfg(target_os = "linux")]
mod service {
    use super::request::{self, Answer, Choice, Filter, Kind, Options};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use zbus::zvariant::{ObjectPath, OwnedValue, Value};
    use zbus::ObjectServer;

    pub const BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.gall";
    pub const OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";

    type Results = HashMap<String, OwnedValue>;

    fn as_bool(v: &Value<'_>) -> Option<bool> {
        match v {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    fn as_string(v: &Value<'_>) -> Option<String> {
        match v {
            Value::Str(s) => Some(s.to_string()),
            _ => None,
        }
    }

    fn as_bytes(v: &Value<'_>) -> Option<Vec<u8>> {
        let Value::Array(a) = v else { return None };
        a.iter()
            .map(|b| match b {
                Value::U8(b) => Some(*b),
                _ => None,
            })
            .collect()
    }

    /// `a(sa(us))`: filters as the portal sends them (kind 0 is a glob, 1 a MIME type).
    fn as_filters(v: &Value<'_>) -> Vec<Filter> {
        let Value::Array(list) = v else { return vec![] };
        list.iter()
            .filter_map(|item| {
                let Value::Structure(s) = item else {
                    return None;
                };
                let [name, rules] = s.fields() else {
                    return None;
                };
                let (Value::Str(name), Value::Array(rules)) = (name, rules) else {
                    return None;
                };
                let mut patterns = Vec::new();
                for rule in rules.iter() {
                    let Value::Structure(r) = rule else { continue };
                    if let [Value::U32(kind), Value::Str(text)] = r.fields() {
                        match kind {
                            0 => patterns.push(text.to_string()),
                            _ => patterns.extend(request::mime_globs(text)),
                        }
                    }
                }
                Some(Filter {
                    name: name.to_string(),
                    patterns,
                })
            })
            .collect()
    }

    /// `a(ssa(ss)s)`: extra options as the portal sends them: id, label, choices (empty: a checkbox), initial.
    fn as_choices(v: &Value<'_>) -> Vec<Choice> {
        let Value::Array(list) = v else { return vec![] };
        list.iter()
            .filter_map(|item| {
                let Value::Structure(s) = item else {
                    return None;
                };
                let [Value::Str(id), Value::Str(label), Value::Array(options), Value::Str(initial)] =
                    s.fields()
                else {
                    return None;
                };
                let options = options
                    .iter()
                    .filter_map(|o| {
                        let Value::Structure(o) = o else { return None };
                        match o.fields() {
                            [Value::Str(oid), Value::Str(olabel)] => {
                                Some((oid.to_string(), olabel.to_string()))
                            }
                            _ => None,
                        }
                    })
                    .collect();
                Some(Choice {
                    id: id.to_string(),
                    label: label.to_string(),
                    options,
                    initial: initial.to_string(),
                })
            })
            .collect()
    }

    pub fn parse_options(opts: &HashMap<String, OwnedValue>) -> Options {
        let get = |k: &str| opts.get(k).map(|v| &**v);
        Options {
            multiple: get("multiple").and_then(as_bool).unwrap_or(false),
            directory: get("directory").and_then(as_bool).unwrap_or(false),
            accept_label: get("accept_label").and_then(as_string),
            current_name: get("current_name").and_then(as_string),
            current_folder: get("current_folder")
                .and_then(as_bytes)
                .and_then(|b| request::path_from_bytes(&b)),
            current_file: get("current_file")
                .and_then(as_bytes)
                .and_then(|b| request::path_from_bytes(&b)),
            filters: get("filters").map(as_filters).unwrap_or_default(),
            choices: get("choices").map(as_choices).unwrap_or_default(),
            modal: get("modal").and_then(as_bool).unwrap_or(false),
            parent_window: None,
            files: match get("files") {
                Some(Value::Array(list)) => list
                    .iter()
                    .filter_map(as_bytes)
                    .filter_map(|b| request::path_from_bytes(&b))
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect(),
                _ => vec![],
            },
        }
    }

    /// The request object the portal may close to cancel the call.
    struct Request {
        closed: Arc<AtomicBool>,
        /// The result file of this call: names the picker window to close.
        result_file: PathBuf,
    }

    #[zbus::interface(name = "org.freedesktop.impl.portal.Request")]
    impl Request {
        fn close(&self) {
            self.closed.store(true, Ordering::SeqCst);
            // The picker window belongs to the program that gave up: it watches for this marker and closes.
            let _ = std::fs::write(request::cancel_marker(&self.result_file), "");
        }
    }

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A folder only this user can use for the result and cancel files: `$XDG_RUNTIME_DIR/gall-portal`,
    /// else a private one in the temp folder. Another user must not be able to plant links or fake answers there.
    fn private_dir() -> std::io::Result<PathBuf> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        let own = std::fs::metadata("/proc/self")?.uid();
        let base = match std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
        {
            Some(runtime) => runtime.join("gall-portal"),
            None => std::env::temp_dir().join(format!("gall-portal-{own}")),
        };
        match std::fs::DirBuilder::new().mode(0o700).create(&base) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let meta = std::fs::symlink_metadata(&base)?;
        if !meta.is_dir() || meta.uid() != own {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("{} is not a folder of this user", base.display()),
            ));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            // Ours, but open to others (made by an older version): close it.
            std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(base)
    }

    fn result_file() -> std::io::Result<PathBuf> {
        Ok(private_dir()?.join(format!(
            "{}-{}.json",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        )))
    }

    fn gall_binary() -> PathBuf {
        std::env::var_os("GALL_PORTAL_GALL")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("gall"))
    }

    /// How long the cancel marker of a withdrawn request is kept for a picker that has not seen it yet.
    const WITHDRAWAL_LIMIT: Duration = Duration::from_secs(600);

    /// Waits until the picker has taken the cancel marker (it deletes it when it closes) or `limit` passed.
    pub fn wait_for_acknowledgement(marker: &Path, limit: Duration) -> bool {
        let start = std::time::Instant::now();
        while marker.exists() {
            if start.elapsed() >= limit {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        true
    }

    /// Starts the picker and waits for its answer (blocking: call from a worker thread).
    fn ask(kind: Kind, title: &str, opts: &Options, closed: &AtomicBool, file: PathBuf) -> Answer {
        let _ = std::fs::remove_file(&file);
        let mut child = match std::process::Command::new(gall_binary())
            .args(request::gall_args(kind, title, opts, &file))
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "xdg-desktop-portal-gall: cannot start {}: {e}",
                    gall_binary().display()
                );
                return Answer {
                    response: request::RESPONSE_OTHER,
                    uris: vec![],
                    filter: None,
                    choices: vec![],
                };
            }
        };
        let answer = loop {
            if closed.load(Ordering::SeqCst) {
                break Answer {
                    response: request::RESPONSE_OTHER,
                    uris: vec![],
                    filter: None,
                    choices: vec![],
                };
            }
            if let Ok(text) = std::fs::read_to_string(&file) {
                break request::parse_answer(&text, kind, &opts.files);
            }
            // `gall` hands the request to the running instance and exits 0: only a failure ends the wait.
            if let Ok(Some(status)) = child.try_wait() {
                if !status.success() && !Path::new(&file).exists() {
                    break Answer {
                        response: request::RESPONSE_OTHER,
                        uris: vec![],
                        filter: None,
                        choices: vec![],
                    };
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let marker = request::cancel_marker(&file);
        let withdrawn = closed.load(Ordering::SeqCst);
        // The child is waited for (no zombie). After a withdrawal the picker window still has to notice the
        // marker (it looks every 300 ms, once it is up; a slow start can take far longer than a minute)
        // and may write a late "cancelled". The picker deletes the marker when it has acted on it, so the
        // files stay until then, up to a generous limit for a picker that never appeared.
        std::thread::spawn(move || {
            let _ = child.wait();
            if withdrawn {
                wait_for_acknowledgement(&marker, WITHDRAWAL_LIMIT);
            }
            let _ = std::fs::remove_file(&file);
            let _ = std::fs::remove_file(marker);
        });
        answer
    }

    fn results(answer: &Answer, filters: &[Filter]) -> Results {
        let mut out = Results::new();
        if answer.response != request::RESPONSE_OK {
            return out;
        }
        if let Ok(v) = OwnedValue::try_from(Value::new(answer.uris.clone())) {
            out.insert("uris".into(), v);
        }
        if !answer.choices.is_empty() {
            if let Ok(v) = OwnedValue::try_from(Value::new(answer.choices.clone())) {
                out.insert("choices".into(), v);
            }
        }
        if let Some(f) = answer.filter.and_then(|i| filters.get(i)) {
            let rules: Vec<(u32, String)> = f.patterns.iter().map(|p| (0u32, p.clone())).collect();
            if let Ok(v) = OwnedValue::try_from(Value::new((f.name.clone(), rules))) {
                out.insert("current_filter".into(), v);
            }
        }
        out
    }

    async fn run(
        server: &ObjectServer,
        handle: ObjectPath<'_>,
        kind: Kind,
        title: String,
        parent_window: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, Results) {
        let mut opts = parse_options(&options);
        opts.parent_window = Some(parent_window).filter(|w| !w.is_empty());
        if std::env::var_os("GALL_PORTAL_DEBUG").is_some() {
            eprintln!("xdg-desktop-portal-gall: {kind:?} options {options:?} -> {opts:?}");
        }
        let closed = Arc::new(AtomicBool::new(false));
        let Ok(file) = result_file().inspect_err(|e| eprintln!("xdg-desktop-portal-gall: {e}"))
        else {
            return (request::RESPONSE_OTHER, Results::new());
        };
        let registered = server
            .at(
                &handle,
                Request {
                    closed: closed.clone(),
                    result_file: file.clone(),
                },
            )
            .await
            .unwrap_or(false);
        let (flag, o2) = (closed.clone(), opts.clone());
        let answer = blocking::unblock(move || ask(kind, &title, &o2, &flag, file)).await;
        if registered {
            let _ = server.remove::<Request, _>(&handle).await;
        }
        (answer.response, results(&answer, &opts.filters))
    }

    pub struct FileChooser;

    #[zbus::interface(name = "org.freedesktop.impl.portal.FileChooser")]
    impl FileChooser {
        async fn open_file(
            &self,
            handle: ObjectPath<'_>,
            _app_id: String,
            parent_window: String,
            title: String,
            options: HashMap<String, OwnedValue>,
            #[zbus(object_server)] server: &ObjectServer,
        ) -> (u32, Results) {
            run(server, handle, Kind::Open, title, parent_window, options).await
        }

        async fn save_file(
            &self,
            handle: ObjectPath<'_>,
            _app_id: String,
            parent_window: String,
            title: String,
            options: HashMap<String, OwnedValue>,
            #[zbus(object_server)] server: &ObjectServer,
        ) -> (u32, Results) {
            run(server, handle, Kind::Save, title, parent_window, options).await
        }

        async fn save_files(
            &self,
            handle: ObjectPath<'_>,
            _app_id: String,
            parent_window: String,
            title: String,
            options: HashMap<String, OwnedValue>,
            #[zbus(object_server)] server: &ObjectServer,
        ) -> (u32, Results) {
            run(
                server,
                handle,
                Kind::SaveFiles,
                title,
                parent_window,
                options,
            )
            .await
        }

        /// Version 3 of the FileChooser interface (choices, current_filter, SaveFiles).
        #[zbus(property, name = "version")]
        fn version(&self) -> u32 {
            3
        }
    }

    pub fn serve() -> zbus::Result<()> {
        let _connection = zbus::blocking::connection::Builder::session()?
            .name(BUS_NAME)?
            .serve_at(OBJECT_PATH, FileChooser)?
            .build()?;
        eprintln!("xdg-desktop-portal-gall: serving {BUS_NAME}");
        loop {
            std::thread::park();
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::service::wait_for_acknowledgement;
    use std::time::Duration;

    #[test]
    fn a_withdrawn_request_keeps_its_marker_until_the_picker_takes_it() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("r.json.cancel");
        std::fs::write(&marker, "").unwrap();
        // Nobody takes it: the wait ends at the limit and reports that it was not acknowledged.
        assert!(!wait_for_acknowledgement(
            &marker,
            Duration::from_millis(250)
        ));
        // A slow-starting picker takes it after the first wait would have been over.
        let m2 = marker.clone();
        let picker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            std::fs::remove_file(m2).unwrap();
        });
        assert!(wait_for_acknowledgement(&marker, Duration::from_secs(5)));
        picker.join().unwrap();
    }
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(e) = service::serve() {
        eprintln!("xdg-desktop-portal-gall: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("xdg-desktop-portal-gall runs on Linux only");
    std::process::exit(1);
}
