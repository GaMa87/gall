//! Linux/Wayland file clipboard (`x-special/gnome-copied-files`, `text/uri-list`,
//! `text/plain;charset=utf-8`) and native GTK drag-and-drop (`text/uri-list`).

use std::path::PathBuf;

/// Encodes `paths` in GNOME's `x-special/gnome-copied-files` clipboard format:
/// `"copy\nfile:///..."` or `"cut\nfile:///..."`.
pub fn encode_gnome_copied_files(paths: &[PathBuf], cut: bool) -> String {
    let header = if cut { "cut" } else { "copy" };
    if paths.is_empty() {
        return header.to_string();
    }
    let mut out = String::from(header);
    for p in paths {
        out.push('\n');
        out.push_str(&gall_core::nav::path::encode_file_uri(p));
    }
    out
}

/// Parses `x-special/gnome-copied-files` bytes into `(paths, cut)`.
/// Accepts `\n` or `\r\n` line endings and strips trailing NUL bytes.
pub fn parse_gnome_copied_files(bytes: &[u8]) -> Option<(Vec<PathBuf>, bool)> {
    let raw = String::from_utf8_lossy(bytes);
    let trimmed = raw.trim_end_matches('\0');
    let mut lines = trimmed.lines().map(str::trim).filter(|l| !l.is_empty());
    let op = lines.next()?;
    let cut = match op {
        "cut" => true,
        "copy" => false,
        _ => return None,
    };
    let paths: Vec<PathBuf> = lines
        .filter_map(gall_core::nav::path::decode_file_uri)
        .collect();
    if paths.is_empty() {
        None
    } else {
        Some((paths, cut))
    }
}

/// Encodes `paths` as an RFC 2483 `text/uri-list` payload (`\r\n` line endings).
pub fn encode_uri_list(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let mut out = paths
        .iter()
        .map(|p| gall_core::nav::path::encode_file_uri(p))
        .collect::<Vec<_>>()
        .join("\r\n");
    out.push_str("\r\n");
    out
}

/// Parses an RFC 2483 `text/uri-list` payload.
/// Accepts both `\r\n` and `\n`, skips blank lines and `#` comments, and keeps only
/// valid local `file://` URIs.
pub fn parse_uri_list(bytes: &[u8]) -> Vec<PathBuf> {
    let raw = String::from_utf8_lossy(bytes);
    raw.trim_end_matches('\0')
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(gall_core::nav::path::decode_file_uri)
        .collect()
}

/// Encodes `paths` as newline-separated plain filesystem paths.
pub fn encode_plain_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(target_os = "linux")]
mod gtk_impl {
    use super::*;
    use gtk::glib;
    use gtk::prelude::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::mpsc;
    use std::time::Duration;
    use tauri::AppHandle;

    const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(2);

    #[derive(Debug, Clone)]
    pub enum GtkClipboardSnapshot {
        GnomeFiles { paths: Vec<PathBuf>, cut: bool },
        UriList { paths: Vec<PathBuf> },
        Text(String),
        Image { width: usize, height: usize },
        Empty,
    }

    /// Writes `paths` to the GTK/Wayland clipboard offering `x-special/gnome-copied-files`,
    /// `text/uri-list` (CRLF), and `text/plain;charset=utf-8` simultaneously.
    /// Returns `Ok(true)` if GTK owned the display and installed the targets, `Ok(false)` if
    /// no GTK display is available (so callers can fall back to `arboard`).
    pub fn set_files(app: &AppHandle, paths: &[PathBuf], cut: bool) -> Result<bool, String> {
        let gnome_bytes = encode_gnome_copied_files(paths, cut).into_bytes();
        let uri_list_bytes = encode_uri_list(paths).into_bytes();
        let plain_text = encode_plain_paths(paths);

        let (tx, rx) = mpsc::channel::<bool>();
        if app
            .run_on_main_thread(move || {
                let Some(display) = gdk::Display::default() else {
                    let _ = tx.send(false);
                    return;
                };
                let cb = gtk::Clipboard::for_display(&display, &gdk::SELECTION_CLIPBOARD);
                let targets = [
                    gtk::TargetEntry::new(
                        "x-special/gnome-copied-files",
                        gtk::TargetFlags::empty(),
                        1,
                    ),
                    gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::empty(), 2),
                    gtk::TargetEntry::new("text/plain;charset=utf-8", gtk::TargetFlags::empty(), 3),
                    gtk::TargetEntry::new("UTF8_STRING", gtk::TargetFlags::empty(), 3),
                    gtk::TargetEntry::new("text/plain", gtk::TargetFlags::empty(), 3),
                ];
                let ok = cb.set_with_data(&targets, move |_, sel, info| match info {
                    1 => {
                        let atom = gdk::Atom::intern("x-special/gnome-copied-files");
                        sel.set(&atom, 8, &gnome_bytes);
                    }
                    2 => {
                        let atom = gdk::Atom::intern("text/uri-list");
                        sel.set(&atom, 8, &uri_list_bytes);
                    }
                    _ => {
                        // Raw bytes under the requested text target: GTK's `set_text` would
                        // convert "\n" to "\r\n" for some targets, which breaks pasting paths.
                        sel.set(&sel.target(), 8, plain_text.as_bytes());
                    }
                });
                let _ = tx.send(ok);
            })
            .is_err()
        {
            return Ok(false);
        }

        // A timeout is an error, not "no GTK": the queued main-thread write may still run, so the
        // caller must not also write through a second clipboard backend.
        rx.recv_timeout(CLIPBOARD_TIMEOUT)
            .map_err(|_| "clipboard write timed out".to_string())
    }

    pub fn set_text(app: &AppHandle, text: &str) -> Result<bool, String> {
        let owned = text.to_string();
        let (tx, rx) = mpsc::channel::<bool>();
        if app
            .run_on_main_thread(move || {
                let Some(display) = gdk::Display::default() else {
                    let _ = tx.send(false);
                    return;
                };
                let cb = gtk::Clipboard::for_display(&display, &gdk::SELECTION_CLIPBOARD);
                cb.set_text(&owned);
                let _ = tx.send(true);
            })
            .is_err()
        {
            return Ok(false);
        }
        rx.recv_timeout(CLIPBOARD_TIMEOUT)
            .map_err(|_| "clipboard write timed out".to_string())
    }

    fn atom_present(atoms: &[gdk::Atom], name: &str) -> bool {
        atoms.iter().any(|a| a.name().as_str() == name)
    }

    fn inspect_step_text_or_image(
        cb: &gtk::Clipboard,
        has_text: bool,
        has_image: bool,
        tx: mpsc::Sender<Option<GtkClipboardSnapshot>>,
    ) {
        if has_text {
            let cb_next = cb.clone();
            let tx_next = tx.clone();
            cb.request_text(move |_, opt_text| {
                if let Some(t) = opt_text {
                    let _ = tx_next.send(Some(GtkClipboardSnapshot::Text(t.to_string())));
                } else if has_image {
                    cb_next.request_image(move |_, opt_pix| {
                        let snap = match opt_pix {
                            Some(p) => GtkClipboardSnapshot::Image {
                                width: p.width().max(0) as usize,
                                height: p.height().max(0) as usize,
                            },
                            None => GtkClipboardSnapshot::Empty,
                        };
                        let _ = tx_next.send(Some(snap));
                    });
                } else {
                    let _ = tx_next.send(Some(GtkClipboardSnapshot::Empty));
                }
            });
            return;
        }
        if has_image {
            cb.request_image(move |_, opt_pix| {
                let snap = match opt_pix {
                    Some(p) => GtkClipboardSnapshot::Image {
                        width: p.width().max(0) as usize,
                        height: p.height().max(0) as usize,
                    },
                    None => GtkClipboardSnapshot::Empty,
                };
                let _ = tx.send(Some(snap));
            });
            return;
        }
        let _ = tx.send(Some(GtkClipboardSnapshot::Empty));
    }

    fn inspect_step_uri_list(
        cb: &gtk::Clipboard,
        has_uri_list: bool,
        has_text: bool,
        has_image: bool,
        tx: mpsc::Sender<Option<GtkClipboardSnapshot>>,
    ) {
        if !has_uri_list {
            inspect_step_text_or_image(cb, has_text, has_image, tx);
            return;
        }
        let cb_next = cb.clone();
        cb.request_contents(&gdk::Atom::intern("text/uri-list"), move |_, sd| {
            if sd.length() > 0 {
                let paths = parse_uri_list(&sd.data());
                if !paths.is_empty() {
                    let _ = tx.send(Some(GtkClipboardSnapshot::UriList { paths }));
                    return;
                }
            }
            inspect_step_text_or_image(&cb_next, has_text, has_image, tx);
        });
    }

    /// Reads the current GTK clipboard asynchronously (non-blocking callbacks with timeout).
    /// Returns `None` when no GTK display is present or no GTK targets are exposed,
    /// allowing the caller to fall back to `arboard`.
    pub fn inspect(app: &AppHandle) -> Option<GtkClipboardSnapshot> {
        let (tx, rx) = mpsc::channel::<Option<GtkClipboardSnapshot>>();
        if app
            .run_on_main_thread(move || {
                let Some(display) = gdk::Display::default() else {
                    let _ = tx.send(None);
                    return;
                };
                let cb = gtk::Clipboard::for_display(&display, &gdk::SELECTION_CLIPBOARD);
                cb.request_contents(&gdk::Atom::intern("TARGETS"), move |cb, sd| {
                    let Some(atoms) = sd.targets() else {
                        let _ = tx.send(None);
                        return;
                    };
                    if atoms.is_empty() {
                        let _ = tx.send(None);
                        return;
                    }
                    let has_gnome = atom_present(&atoms, "x-special/gnome-copied-files");
                    let has_uri_list = atom_present(&atoms, "text/uri-list");
                    let has_text = atom_present(&atoms, "text/plain;charset=utf-8")
                        || atom_present(&atoms, "UTF8_STRING")
                        || atom_present(&atoms, "text/plain")
                        || atom_present(&atoms, "TEXT");
                    let has_image = sd.targets_include_image(false)
                        || atoms.iter().any(|a| a.name().starts_with("image/"));

                    if has_gnome {
                        let cb_next = cb.clone();
                        let tx_next = tx.clone();
                        cb.request_contents(
                            &gdk::Atom::intern("x-special/gnome-copied-files"),
                            move |_, gnome_sd| {
                                if gnome_sd.length() > 0 {
                                    if let Some((paths, cut)) =
                                        parse_gnome_copied_files(&gnome_sd.data())
                                    {
                                        let _ =
                                            tx_next.send(Some(GtkClipboardSnapshot::GnomeFiles {
                                                paths,
                                                cut,
                                            }));
                                        return;
                                    }
                                }
                                inspect_step_uri_list(
                                    &cb_next,
                                    has_uri_list,
                                    has_text,
                                    has_image,
                                    tx_next,
                                );
                            },
                        );
                    } else {
                        inspect_step_uri_list(cb, has_uri_list, has_text, has_image, tx);
                    }
                });
            })
            .is_err()
        {
            return None;
        }
        rx.recv_timeout(CLIPBOARD_TIMEOUT).ok().flatten()
    }

    /// Raw payload of each file-related target of the GTK clipboard (self-test wire-format check).
    pub fn dump(app: &AppHandle) -> Option<Vec<(String, Vec<u8>)>> {
        const TARGETS: [&str; 3] = [
            "x-special/gnome-copied-files",
            "text/uri-list",
            "text/plain;charset=utf-8",
        ];
        let (tx, rx) = mpsc::channel::<(String, Vec<u8>)>();
        app.run_on_main_thread(move || {
            let Some(display) = gdk::Display::default() else {
                return;
            };
            let cb = gtk::Clipboard::for_display(&display, &gdk::SELECTION_CLIPBOARD);
            for target in TARGETS {
                let tx = tx.clone();
                cb.request_contents(&gdk::Atom::intern(target), move |_, sd| {
                    let data = if sd.length() > 0 {
                        sd.data()
                    } else {
                        Vec::new()
                    };
                    let _ = tx.send((target.to_string(), data));
                });
            }
        })
        .ok()?;
        let mut out = Vec::new();
        while out.len() < TARGETS.len() {
            match rx.recv_timeout(CLIPBOARD_TIMEOUT) {
                Ok(item) => out.push(item),
                Err(_) => break,
            }
        }
        Some(out)
    }

    pub fn read_files(app: &AppHandle) -> Option<(Vec<PathBuf>, bool)> {
        match inspect(app)? {
            GtkClipboardSnapshot::GnomeFiles { paths, cut } => Some((paths, cut)),
            GtkClipboardSnapshot::UriList { paths } => Some((paths, false)),
            _ => None,
        }
    }

    pub fn read_text(app: &AppHandle) -> Option<String> {
        let (tx, rx) = mpsc::channel::<Option<String>>();
        if app
            .run_on_main_thread(move || {
                let Some(display) = gdk::Display::default() else {
                    let _ = tx.send(None);
                    return;
                };
                let cb = gtk::Clipboard::for_display(&display, &gdk::SELECTION_CLIPBOARD);
                cb.request_text(move |_, opt| {
                    let _ = tx.send(opt.map(str::to_string));
                });
            })
            .is_err()
        {
            return None;
        }
        rx.recv_timeout(CLIPBOARD_TIMEOUT).ok().flatten()
    }

    pub fn read_image_png(app: &AppHandle) -> Option<Vec<u8>> {
        let (tx, rx) = mpsc::channel::<Option<Vec<u8>>>();
        if app
            .run_on_main_thread(move || {
                let Some(display) = gdk::Display::default() else {
                    let _ = tx.send(None);
                    return;
                };
                let cb = gtk::Clipboard::for_display(&display, &gdk::SELECTION_CLIPBOARD);
                cb.request_image(move |_, opt_pix| {
                    let bytes = opt_pix.and_then(|pix| pix.save_to_bufferv("png", &[]).ok());
                    let _ = tx.send(bytes);
                });
            })
            .is_err()
        {
            return None;
        }
        rx.recv_timeout(CLIPBOARD_TIMEOUT).ok().flatten()
    }

    struct ActiveDrag {
        widget: glib::WeakRef<gtk::Widget>,
        handlers: Vec<glib::SignalHandlerId>,
    }

    thread_local! {
        static ACTIVE_DRAG: RefCell<Option<ActiveDrag>> = const { RefCell::new(None) };
    }

    fn cleanup_active_drag(current_widget: &gtk::Widget) {
        ACTIVE_DRAG.with(|slot| {
            if let Some(drag) = slot.borrow_mut().take() {
                if let Some(w) = drag.widget.upgrade() {
                    for id in drag.handlers {
                        w.disconnect(id);
                    }
                } else {
                    for id in drag.handlers {
                        current_widget.disconnect(id);
                    }
                }
            }
        });
    }

    pub async fn start_native_drag(
        win: &tauri::WebviewWindow,
        paths: Vec<PathBuf>,
    ) -> Result<String, String> {
        if paths.is_empty() {
            return Err("nothing to drag".into());
        }
        let uri_list_bytes = encode_uri_list(&paths).into_bytes();
        let (tx, rx) = mpsc::channel::<Result<String, String>>();

        win.with_webview(move |webview| {
            let widget: gtk::Widget = webview.inner().upcast();
            cleanup_active_drag(&widget);

            let failed = Rc::new(Cell::new(false));
            let tx_cell = Rc::new(RefCell::new(Some(tx)));

            let data_bytes = uri_list_bytes.clone();
            let data_get_id = widget.connect_drag_data_get(move |_, _, sel, _, _| {
                let atom = gdk::Atom::intern("text/uri-list");
                sel.set(&atom, 8, &data_bytes);
            });

            let failed_flag = failed.clone();
            let failed_id = widget.connect_drag_failed(move |_, _, _| {
                failed_flag.set(true);
                glib::Propagation::Stop
            });

            let failed_end = failed.clone();
            let tx_end = tx_cell.clone();
            let end_id = widget.connect_drag_end(move |w, ctx| {
                cleanup_active_drag(w);
                let outcome = if failed_end.get() || ctx.selected_action().is_empty() {
                    "cancelled"
                } else {
                    "dropped"
                };
                if let Some(sender) = tx_end.borrow_mut().take() {
                    let _ = sender.send(Ok(outcome.to_string()));
                }
            });

            ACTIVE_DRAG.with(|slot| {
                *slot.borrow_mut() = Some(ActiveDrag {
                    widget: widget.downgrade(),
                    handlers: vec![data_get_id, failed_id, end_id],
                });
            });

            let target_entry = gtk::TargetEntry::new("text/uri-list", gtk::TargetFlags::empty(), 0);
            let targets = gtk::TargetList::new(&[target_entry]);
            let event = gtk::current_event();
            let ctx = widget.drag_begin_with_coordinates(
                &targets,
                gdk::DragAction::COPY | gdk::DragAction::MOVE,
                1,
                event.as_ref(),
                -1,
                -1,
            );
            if ctx.is_none() {
                cleanup_active_drag(&widget);
                if let Some(sender) = tx_cell.borrow_mut().take() {
                    let _ = sender.send(Err("could not start native drag".into()));
                }
            }
        })
        .map_err(|err| err.to_string())?;

        tauri::async_runtime::spawn_blocking(move || rx.recv().unwrap_or(Err("drag ended".into())))
            .await
            .map_err(|err| err.to_string())?
    }
}

#[cfg(target_os = "linux")]
pub use gtk_impl::{
    dump, inspect, read_files, read_image_png, read_text, set_files, set_text, start_native_drag,
    GtkClipboardSnapshot,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_copied_files_round_trips_copy_and_cut_with_spaces_and_unicode() {
        let paths = vec![
            PathBuf::from("/tmp/My Folder/Črka #1?a.txt"),
            PathBuf::from("/var/log/syslog"),
        ];

        let copy_payload = encode_gnome_copied_files(&paths, false);
        assert!(copy_payload.starts_with("copy\n"));
        let (parsed_copy, cut_copy) = parse_gnome_copied_files(copy_payload.as_bytes()).unwrap();
        assert_eq!(parsed_copy, paths);
        assert!(!cut_copy);

        let cut_payload = encode_gnome_copied_files(&paths, true);
        assert!(cut_payload.starts_with("cut\n"));
        let (parsed_cut, cut_flag) = parse_gnome_copied_files(cut_payload.as_bytes()).unwrap();
        assert_eq!(parsed_cut, paths);
        assert!(cut_flag);
    }

    #[test]
    fn parse_gnome_copied_files_handles_crlf_trailing_nul_and_rejects_invalid() {
        let with_crlf_nul = b"cut\r\nfile:///tmp/a%20b.txt\r\n\r\n\0";
        let (paths, cut) = parse_gnome_copied_files(with_crlf_nul).unwrap();
        assert_eq!(paths, vec![PathBuf::from("/tmp/a b.txt")]);
        assert!(cut);

        assert_eq!(parse_gnome_copied_files(b"move\nfile:///tmp/a"), None);
        assert_eq!(parse_gnome_copied_files(b"copy\n"), None);
        assert_eq!(
            parse_gnome_copied_files(b"copy\nhttps://example.com/a"),
            None
        );
    }

    #[test]
    fn uri_list_encodes_with_crlf_and_parses_crlf_lf_comments_and_blank_lines() {
        let paths = vec![
            PathBuf::from("/home/u/first file.png"),
            PathBuf::from("/home/u/Črka.txt"),
        ];
        let encoded = encode_uri_list(&paths);
        assert_eq!(
            encoded,
            "file:///home/u/first%20file.png\r\nfile:///home/u/%C4%8Crka.txt\r\n"
        );
        assert_eq!(parse_uri_list(encoded.as_bytes()), paths);

        let mixed = b"# comment from external tool\r\n\nfile:///tmp/one\nhttps://ignored.invalid/x\r\nfile://localhost/tmp/two%20b\0";
        assert_eq!(
            parse_uri_list(mixed),
            vec![PathBuf::from("/tmp/one"), PathBuf::from("/tmp/two b")]
        );
        assert!(encode_uri_list(&[]).is_empty());
    }

    #[test]
    fn encode_plain_paths_joins_with_newlines() {
        let paths = vec![PathBuf::from("/a/one"), PathBuf::from("/b/two three")];
        assert_eq!(encode_plain_paths(&paths), "/a/one\n/b/two three");
    }
}
