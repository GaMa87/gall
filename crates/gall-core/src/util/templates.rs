//! New-file templates (O-08): any file in the templates folder is cloned.

use crate::error::{IoContext, Result};
use crate::fs::ops::unique_path;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    pub name: String,
    pub path: String,
    /// Suggested base name for the new file ("New Text File.txt").
    pub suggested: String,
}

/// Built-in starter templates, written on first run. Our own minimal content.
const SEED: &[(&str, &str)] = &[
    ("Text File.txt", ""),
    ("Markdown.md", "# Title\n\n"),
    ("Shell Script.sh", "#!/usr/bin/env bash\nset -euo pipefail\n\n"),
    ("Python Script.py", "#!/usr/bin/env python3\n\n\ndef main():\n    pass\n\n\nif __name__ == \"__main__\":\n    main()\n"),
    ("Web Page.html", "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\">\n  <title></title>\n</head>\n<body>\n\n</body>\n</html>\n"),
];

pub fn ensure_seeded(dir: &Path) -> Result<()> {
    if dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).ctx("create folder", dir)?;
    for (name, body) in SEED {
        std::fs::write(dir.join(name), body).ctx("write", dir.join(name))?;
    }
    Ok(())
}

pub fn list(dir: &Path) -> Result<Vec<Template>> {
    let mut v: Vec<Template> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                Template {
                    suggested: format!("New {name}"),
                    path: e.path().to_string_lossy().into_owned(),
                    name,
                }
            })
            .collect(),
        Err(_) => vec![],
    };
    v.sort_by_key(|t| t.name.to_lowercase());
    Ok(v)
}

/// "Create file template" from any file: copy it into the templates folder.
pub fn add_from(dir: &Path, file: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).ctx("create folder", dir)?;
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "template".into());
    let dest = unique_path(dir, &name);
    std::fs::copy(file, &dest).ctx("copy", file)?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_list_add() {
        let d = tempfile::tempdir().unwrap();
        let t = d.path().join("templates");
        ensure_seeded(&t).unwrap();
        let l = list(&t).unwrap();
        assert!(l
            .iter()
            .any(|x| x.name == "Text File.txt" && x.suggested == "New Text File.txt"));
        let f = d.path().join("letter.odt");
        std::fs::write(&f, "x").unwrap();
        add_from(&t, &f).unwrap();
        assert!(list(&t).unwrap().iter().any(|x| x.name == "letter.odt"));
    }
}
