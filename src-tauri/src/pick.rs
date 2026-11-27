//! Picker windows (`gall --pick`): the result of the user's choice goes to a JSON file that the
//! caller (the `xdg-desktop-portal` FileChooser backend) waits for.

use crate::cli::PickChoice;
use serde::Serialize;
use std::path::Path;

/// `status` follows the portal's `response` codes: 0 chosen, 1 cancelled.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct PickResult<'a> {
    pub status: u32,
    pub paths: &'a [String],
    /// Index of the filter that was active (into `--pick-filter` order).
    pub filter: Option<usize>,
    /// `[id, value]` of every choice the user answered (the portal's `choices`).
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub choices: &'a [(String, String)],
}

/// What the picker window reports for the choices, made safe: only known ids, in the order the caller
/// asked, and a combo value that is not one of its options falls back to the initial one; a checkbox is
/// `"true"` or `"false"`.
pub fn sanitize_choices(asked: &[PickChoice], given: &[(String, String)]) -> Vec<(String, String)> {
    asked
        .iter()
        .map(|c| {
            let value = given
                .iter()
                .find(|(id, _)| *id == c.id)
                .map(|(_, v)| v.as_str());
            let ok = |v: &str| {
                if c.options.is_empty() {
                    v == "true" || v == "false"
                } else {
                    c.options.iter().any(|(id, _)| id == v)
                }
            };
            let chosen = match value {
                Some(v) if ok(v) => v.to_string(),
                _ if ok(&c.initial) => c.initial.clone(),
                _ if c.options.is_empty() => "false".to_string(),
                _ => c
                    .options
                    .first()
                    .map(|(id, _)| id.clone())
                    .unwrap_or_default(),
            };
            (c.id.clone(), chosen)
        })
        .collect()
}

/// The file the caller creates to withdraw the request: the picker window then closes (cancelled).
pub fn cancel_marker(result: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{result}.cancel"))
}

/// Writes `bytes` to a new file only the user can read (mode 0600 on Unix), refusing to follow a
/// planted link: the temp name is unique and must not exist yet.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    // Unique per call: two threads of this process writing the same file must not share a temp file.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("{}.{seq}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let _ = std::fs::remove_file(&tmp); // a leftover of an earlier run of this process id
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    drop(file);
    std::fs::rename(&tmp, path)
}

/// Writes the result atomically (private temp file + rename), so the reader never sees half of it.
pub fn write_result(path: &str, result: &PickResult<'_>) -> std::io::Result<()> {
    write_private(
        Path::new(path),
        &serde_json::to_vec(result).unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_are_checked_against_what_was_asked() {
        let asked = vec![
            PickChoice {
                id: "enc".into(),
                label: "Encoding".into(),
                options: vec![
                    ("u8".into(), "UTF-8".into()),
                    ("l1".into(), "Latin-1".into()),
                ],
                initial: "l1".into(),
            },
            PickChoice {
                id: "ow".into(),
                label: "Overwrite".into(),
                options: vec![],
                initial: "true".into(),
            },
            PickChoice {
                id: "bad".into(),
                label: "Bad initial".into(),
                options: vec![("a".into(), "A".into())],
                initial: "zzz".into(),
            },
        ];
        let given = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect()
        };
        // Valid answers pass; unknown ids are dropped; the caller's order is kept.
        assert_eq!(
            sanitize_choices(
                &asked,
                &given(&[("ow", "false"), ("evil", "x"), ("enc", "u8"), ("bad", "a")])
            ),
            given(&[("enc", "u8"), ("ow", "false"), ("bad", "a")])
        );
        // Missing or invalid values fall back to the initial one, else to something valid.
        assert_eq!(
            sanitize_choices(&asked, &given(&[("enc", "../../etc"), ("ow", "maybe")])),
            given(&[("enc", "l1"), ("ow", "true"), ("bad", "a")])
        );
    }

    #[test]
    fn the_cancel_marker_sits_next_to_the_result() {
        assert_eq!(
            cancel_marker("/run/u/r.json"),
            std::path::PathBuf::from("/run/u/r.json.cancel")
        );
    }

    #[test]
    fn results_are_written_whole_and_readable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("r.json");
        let paths = vec!["/a/b c.txt".to_string(), "/a/\u{010d}.md".to_string()];
        write_result(
            file.to_str().unwrap(),
            &PickResult {
                status: 0,
                paths: &paths,
                filter: Some(1),
                choices: &[],
            },
        )
        .unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["status"], 0);
        assert_eq!(v["paths"][1], "/a/\u{010d}.md");
        assert_eq!(v["filter"], 1);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "no temp file is left behind"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&file).unwrap().permissions().mode() & 0o077,
                0,
                "only the user may read it"
            );
        }
        write_result(
            file.to_str().unwrap(),
            &PickResult {
                status: 1,
                paths: &[],
                filter: None,
                choices: &[],
            },
        )
        .unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(
            (v["status"].clone(), v["filter"].clone()),
            (1.into(), serde_json::Value::Null)
        );
    }
}
