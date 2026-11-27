//! What a FileChooser call asks for, as arguments of `gall --pick`, and how Gall's answer becomes the
//! portal's reply. Pure functions: the D-Bus side is in `main.rs`.

use serde_json::Value;
use std::path::{Path, PathBuf};

/// `OpenFile`, `SaveFile` or `SaveFiles`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Open,
    Save,
    SaveFiles,
}

/// One file-type filter: a name and globs (`*.png`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Filter {
    pub name: String,
    pub patterns: Vec<String>,
}

/// One extra option of a dialog (`choices`): a checkbox (no `options`, value `"true"`/`"false"`) or
/// a combo box (value is one of the option ids).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Choice {
    pub id: String,
    pub label: String,
    pub options: Vec<(String, String)>,
    pub initial: String,
}

/// The options of a call that matter to Gall (the rest of `a{sv}` is ignored).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    pub multiple: bool,
    pub directory: bool,
    pub accept_label: Option<String>,
    pub current_name: Option<String>,
    pub current_folder: Option<PathBuf>,
    pub current_file: Option<PathBuf>,
    pub filters: Vec<Filter>,
    pub choices: Vec<Choice>,
    /// `SaveFiles`: the names of the files to save.
    pub files: Vec<String>,
    /// `modal` and the caller's window handle. Gall's picker is always a window of its own (a
    /// Wayland client cannot parent a window to another program's without xdg-foreign), so these
    /// are recorded for diagnostics only.
    pub modal: bool,
    pub parent_window: Option<String>,
}

/// Globs for a MIME type filter entry; unknown types give none (the filter then shows everything).
pub fn mime_globs(mime: &str) -> Vec<String> {
    let exts: &[&str] = match mime {
        "image/*" => &[
            "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "tif", "tiff", "avif", "heic",
        ],
        "video/*" => &["mp4", "mkv", "webm", "mov", "avi", "m4v", "ogv"],
        "audio/*" => &["mp3", "flac", "ogg", "opus", "wav", "m4a", "aac"],
        "image/png" => &["png"],
        "image/jpeg" => &["jpg", "jpeg"],
        "image/gif" => &["gif"],
        "image/webp" => &["webp"],
        "image/svg+xml" => &["svg"],
        "text/plain" => &["txt", "md", "log"],
        "text/html" => &["html", "htm"],
        "text/csv" => &["csv"],
        "application/pdf" => &["pdf"],
        "application/zip" => &["zip"],
        "application/json" => &["json"],
        "video/mp4" => &["mp4"],
        "audio/mpeg" => &["mp3"],
        _ => &[],
    };
    exts.iter().map(|e| format!("*.{e}")).collect()
}

/// A portal path option: a NUL-terminated byte string.
pub fn path_from_bytes(bytes: &[u8]) -> Option<PathBuf> {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    if end == 0 {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes[..end])))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes[..end].to_vec())
            .ok()
            .map(PathBuf::from)
    }
}

/// `file://` URI of an absolute path, percent-encoded as the portal protocol expects.
pub fn file_uri(path: &Path) -> String {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    };
    #[cfg(not(unix))]
    let bytes = path.to_string_lossy().into_owned().into_bytes();
    let mut out = String::from("file://");
    for b in bytes {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Where the picker starts: the caller's folder, else the folder of its current file.
pub fn start_folder(o: &Options) -> Option<PathBuf> {
    o.current_folder
        .clone()
        .or_else(|| {
            o.current_file
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
        })
        .filter(|p| p.is_absolute())
}

/// The choices as the JSON `--pick-choices` takes: `[{"id","label","options":[["id","label"]],"initial"}]`.
pub fn choices_json(choices: &[Choice]) -> String {
    let list: Vec<Value> = choices
        .iter()
        .map(|c| {
            serde_json::json!({
                "id": c.id,
                "label": c.label,
                "options": c.options.iter().map(|(i, l)| [i, l]).collect::<Vec<_>>(),
                "initial": c.initial,
            })
        })
        .collect();
    Value::Array(list).to_string()
}

/// Arguments for `gall` (after the program name) that open the picker.
pub fn gall_args(kind: Kind, title: &str, o: &Options, result_file: &Path) -> Vec<String> {
    let mode = match kind {
        Kind::Open if o.directory => "dir",
        Kind::Open => "open",
        Kind::Save => "save",
        Kind::SaveFiles => "dir",
    };
    let mut a = vec![
        "--pick".to_string(),
        mode.to_string(),
        "--pick-result".into(),
        result_file.to_string_lossy().into_owned(),
    ];
    if !title.is_empty() {
        a.extend(["--pick-title".into(), title.to_string()]);
    }
    if o.multiple && kind == Kind::Open {
        a.push("--pick-multiple".into());
    }
    if let Some(name) = &o.current_name {
        a.extend(["--pick-name".into(), name.clone()]);
    } else if kind == Kind::Save {
        if let Some(name) = o.current_file.as_deref().and_then(Path::file_name) {
            a.extend(["--pick-name".into(), name.to_string_lossy().into_owned()]);
        }
    }
    if let Some(l) = &o.accept_label {
        a.extend(["--pick-accept".into(), l.clone()]);
    }
    if kind != Kind::SaveFiles {
        for f in &o.filters {
            a.extend([
                "--pick-filter".into(),
                format!("{}|{}", f.name.replace('|', "/"), f.patterns.join(";")),
            ]);
        }
    }
    if !o.choices.is_empty() {
        a.extend(["--pick-choices".into(), choices_json(&o.choices)]);
    }
    // `--` first: a folder named like an option stays a folder.
    a.push("--".into());
    if let Some(dir) = start_folder(o) {
        a.push(dir.to_string_lossy().into_owned());
    }
    a
}

/// The file whose creation withdraws the request: the picker window answering into `result_file` closes.
pub fn cancel_marker(result_file: &Path) -> PathBuf {
    let mut name = result_file.as_os_str().to_os_string();
    name.push(".cancel");
    PathBuf::from(name)
}

/// The portal `response` codes.
pub const RESPONSE_OK: u32 = 0;
pub const RESPONSE_CANCELLED: u32 = 1;
pub const RESPONSE_OTHER: u32 = 2;

/// What Gall wrote to the result file.
#[derive(Debug, PartialEq, Eq)]
pub struct Answer {
    pub response: u32,
    pub uris: Vec<String>,
    /// Index into the call's filters.
    pub filter: Option<usize>,
    /// `(choice id, selected value)` for every choice the picker returned.
    pub choices: Vec<(String, String)>,
}

/// Reads the result file of `gall --pick`; for `SaveFiles` the chosen folder is joined with `names`.
pub fn parse_answer(json: &str, kind: Kind, names: &[String]) -> Answer {
    let other = Answer {
        response: RESPONSE_OTHER,
        uris: vec![],
        filter: None,
        choices: vec![],
    };
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return other;
    };
    match v.get("status").and_then(Value::as_u64) {
        Some(0) => {}
        Some(1) => {
            return Answer {
                response: RESPONSE_CANCELLED,
                ..other
            }
        }
        _ => return other,
    }
    let paths: Vec<PathBuf> = v
        .get("paths")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default();
    if paths.is_empty() {
        return other;
    }
    let uris = if kind == Kind::SaveFiles {
        // Only the last part of each name: an absolute name or `..` must not leave the chosen folder.
        names
            .iter()
            .filter_map(|n| Path::new(n).file_name())
            .map(|n| file_uri(&paths[0].join(n)))
            .collect()
    } else {
        paths.iter().map(|p| file_uri(p)).collect()
    };
    Answer {
        response: RESPONSE_OK,
        uris,
        filter: v.get("filter").and_then(Value::as_u64).map(|i| i as usize),
        choices: v
            .get("choices")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|pair| {
                        let pair = pair.as_array()?;
                        Some((
                            pair.first()?.as_str()?.to_string(),
                            pair.get(1)?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

// The portal runs on Linux only; the tests use Unix paths.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn uris_are_percent_encoded() {
        assert_eq!(
            file_uri(Path::new("/home/u/a b#1.txt")),
            "file:///home/u/a%20b%231.txt"
        );
        assert_eq!(file_uri(Path::new("/tmp/\u{010d}")), "file:///tmp/%C4%8D");
        assert_eq!(file_uri(Path::new("/a/100%/x")), "file:///a/100%25/x");
    }

    #[test]
    fn paths_come_from_nul_terminated_bytes() {
        assert_eq!(path_from_bytes(b"/tmp/x\0"), Some(PathBuf::from("/tmp/x")));
        assert_eq!(path_from_bytes(b"/tmp/y"), Some(PathBuf::from("/tmp/y")));
        assert_eq!(path_from_bytes(b"\0"), None);
        assert_eq!(path_from_bytes(b""), None);
    }

    #[test]
    fn mime_filters_become_globs() {
        assert!(mime_globs("image/*").contains(&"*.png".to_string()));
        assert_eq!(mime_globs("application/pdf"), ["*.pdf"]);
        assert!(mime_globs("application/x-unknown").is_empty());
    }

    #[test]
    fn choices_go_to_the_picker_as_json_and_come_back_in_the_answer() {
        let o = Options {
            choices: vec![
                Choice {
                    id: "encoding".into(),
                    label: "Encoding".into(),
                    options: vec![
                        ("utf8".into(), "UTF-8".into()),
                        ("l1".into(), "Latin-1".into()),
                    ],
                    initial: "utf8".into(),
                },
                Choice {
                    id: "overwrite".into(),
                    label: "Overwrite \"quietly\"".into(),
                    options: vec![],
                    initial: "false".into(),
                },
            ],
            ..Default::default()
        };
        let a = gall_args(Kind::Open, "", &o, Path::new("/run/r.json"));
        let at = a.iter().position(|x| x == "--pick-choices").expect("flag");
        let json: Value = serde_json::from_str(&a[at + 1]).unwrap();
        assert_eq!(json[0]["id"], "encoding");
        assert_eq!(json[0]["options"][1][1], "Latin-1");
        assert_eq!(json[1]["label"], "Overwrite \"quietly\"");
        assert_eq!(json[1]["options"].as_array().unwrap().len(), 0);
        assert!(
            a.iter().position(|x| x == "--").unwrap() > at,
            "before the folder"
        );
        assert!(
            !gall_args(Kind::Open, "", &Options::default(), Path::new("/r"))
                .contains(&"--pick-choices".to_string())
        );

        let ans = parse_answer(
            r#"{"status":0,"paths":["/a/b"],"filter":null,"choices":[["encoding","l1"],["overwrite","true"],[1,2]]}"#,
            Kind::Open,
            &[],
        );
        assert_eq!(ans.response, RESPONSE_OK);
        assert_eq!(
            ans.choices,
            [
                ("encoding".to_string(), "l1".to_string()),
                ("overwrite".to_string(), "true".to_string())
            ]
        );
        let none = parse_answer(r#"{"status":0,"paths":["/a/b"]}"#, Kind::Open, &[]);
        assert!(none.choices.is_empty());
    }

    #[test]
    fn open_arguments() {
        let o = Options {
            multiple: true,
            accept_label: Some("_Open".into()),
            current_folder: Some("/home/u/Pictures".into()),
            filters: vec![
                Filter {
                    name: "Images".into(),
                    patterns: vec!["*.png".into(), "*.jpg".into()],
                },
                Filter {
                    name: "All".into(),
                    patterns: vec![],
                },
            ],
            ..Default::default()
        };
        let a = gall_args(Kind::Open, "Select pictures", &o, Path::new("/run/r.json"));
        assert_eq!(
            a,
            [
                "--pick",
                "open",
                "--pick-result",
                "/run/r.json",
                "--pick-title",
                "Select pictures",
                "--pick-multiple",
                "--pick-accept",
                "_Open",
                "--pick-filter",
                "Images|*.png;*.jpg",
                "--pick-filter",
                "All|",
                "--",
                "/home/u/Pictures"
            ]
        );
    }

    #[test]
    fn folder_save_and_save_files_modes() {
        let dir = Options {
            directory: true,
            multiple: true,
            ..Default::default()
        };
        let a = gall_args(Kind::Open, "", &dir, Path::new("/r"));
        assert_eq!(&a[..2], ["--pick", "dir"]);
        assert!(a.contains(&"--pick-multiple".to_string()));
        let save = Options {
            current_file: Some("/home/u/Docs/report.odt".into()),
            ..Default::default()
        };
        let a = gall_args(Kind::Save, "Save", &save, Path::new("/r"));
        assert_eq!(&a[..2], ["--pick", "save"]);
        assert!(a.windows(2).any(|w| w == ["--pick-name", "report.odt"]));
        assert_eq!(
            a.last().unwrap(),
            "/home/u/Docs",
            "starts in the folder of the current file"
        );
        assert_eq!(
            &gall_args(Kind::SaveFiles, "", &Options::default(), Path::new("/r"))[..2],
            ["--pick", "dir"]
        );
        // A relative start folder is ignored.
        assert_eq!(
            gall_args(
                Kind::Open,
                "",
                &Options {
                    current_folder: Some("rel".into()),
                    ..Default::default()
                },
                Path::new("/r")
            )
            .last()
            .unwrap(),
            "--"
        );
    }

    #[test]
    fn closing_a_request_drops_a_marker_next_to_the_result_file() {
        assert_eq!(
            cancel_marker(Path::new("/run/u/gall-portal/1-0.json")),
            Path::new("/run/u/gall-portal/1-0.json.cancel")
        );
    }

    #[test]
    fn save_files_names_cannot_leave_the_chosen_folder() {
        let names: Vec<String> = [
            "/home/u/.bashrc",
            "../../x",
            "a b.txt",
            "",
            "..",
            "sub/c.txt",
        ]
        .map(String::from)
        .to_vec();
        let a = parse_answer(r#"{"status":0,"paths":["/d"]}"#, Kind::SaveFiles, &names);
        assert_eq!(
            a.uris,
            [
                "file:///d/.bashrc",
                "file:///d/x",
                "file:///d/a%20b.txt",
                "file:///d/c.txt"
            ]
        );
    }

    #[test]
    fn answers_map_to_responses() {
        let ok = parse_answer(
            r#"{"status":0,"paths":["/a/b c.txt"],"filter":1}"#,
            Kind::Open,
            &[],
        );
        assert_eq!(
            ok,
            Answer {
                response: 0,
                uris: vec!["file:///a/b%20c.txt".into()],
                filter: Some(1),
                choices: vec![],
            }
        );
        assert_eq!(
            parse_answer(r#"{"status":1,"paths":[]}"#, Kind::Open, &[]).response,
            RESPONSE_CANCELLED
        );
        assert_eq!(
            parse_answer("not json", Kind::Open, &[]).response,
            RESPONSE_OTHER
        );
        assert_eq!(
            parse_answer(r#"{"status":0,"paths":[]}"#, Kind::Save, &[]).response,
            RESPONSE_OTHER
        );
        let many = parse_answer(
            r#"{"status":0,"paths":["/d"]}"#,
            Kind::SaveFiles,
            &["a.txt".into(), "b c.txt".into()],
        );
        assert_eq!(many.uris, ["file:///d/a.txt", "file:///d/b%20c.txt"]);
    }
}
