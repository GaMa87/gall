//! Images of a repository as files, to compare them: a version from a commit or the index is
//! written to a private folder under its object ID, so it is read from Git once.
use super::gitgraph::root;
use super::gitinspect::{commit_id, read_git, relative_file};
use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Larger images are not shown.
pub const MAX_IMAGE: usize = 24 * 1024 * 1024;
/// Files kept in the folder.
const KEEP: usize = 64;
const EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "avif", "svg", "ico",
];

/// Where one side of the comparison is.
#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Side {
    Commit {
        id: String,
        file: String,
    },
    Index {
        file: String,
    },
    Head {
        file: String,
    },
    /// The file in the work tree.
    Work {
        file: String,
    },
    /// The image was added or deleted.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Image {
    /// A file to show; None when there is no image on this side.
    pub path: Option<PathBuf>,
    pub size: u64,
    /// Why it is not shown.
    pub reason: Option<String>,
}

fn fail(message: impl Into<String>) -> CoreError {
    CoreError::Other(message.into())
}

fn extension(file: &str) -> Result<String> {
    let ext = crate::fs::entry::extension_of(file.rsplit('/').next().unwrap_or(file));
    EXTENSIONS
        .contains(&ext.as_str())
        .then_some(ext)
        .ok_or_else(|| fail("Not an image"))
}

/// The folder for images in the cache of this user (a shared `/tmp` could hold another user's). The
/// files are named by their content and at most [`KEEP`] stay, so it never grows.
fn folder() -> Result<PathBuf> {
    let dir = dirs::cache_dir()
        .ok_or_else(|| fail("There is no cache folder"))?
        .join("gall")
        .join("git-images");
    std::fs::create_dir_all(&dir).map_err(|e| fail(e.to_string()))?;
    Ok(dir)
}
/// Removes the oldest files beyond [`KEEP`].
fn trim(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    if files.len() <= KEEP {
        return;
    }
    files.sort();
    for (_, path) in files.iter().take(files.len() - KEEP) {
        let _ = std::fs::remove_file(path);
    }
}

fn blob(root: &Path, spec: &str, ext: &str) -> Result<Image> {
    let (id, _) = read_git(root, &["rev-parse", "--verify", "-q", spec], 4096)?;
    let id = String::from_utf8_lossy(&id).trim().to_owned();
    if !matches!(id.len(), 40 | 64) || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(fail("The image is not in this version"));
    }
    let dir = folder()?;
    let place = dir.join(format!("{id}.{ext}"));
    if let Ok(meta) = place.metadata() {
        return Ok(Image {
            path: Some(place),
            size: meta.len(),
            reason: None,
        });
    }
    let (bytes, large) = read_git(root, &["cat-file", "blob", &id], MAX_IMAGE)?;
    if large {
        return Ok(Image {
            path: None,
            size: 0,
            reason: Some(format!(
                "Images over {} MiB are not shown",
                MAX_IMAGE / 1024 / 1024
            )),
        });
    }
    let size = bytes.len() as u64;
    // Written beside and renamed: another window may look at it meanwhile.
    let part = dir.join(format!("{id}.{:?}.part", std::thread::current().id()));
    std::fs::write(&part, bytes)
        .and_then(|()| std::fs::rename(&part, &place))
        .map_err(|e| {
            let _ = std::fs::remove_file(&part);
            fail(e.to_string())
        })?;
    trim(&dir);
    Ok(Image {
        path: Some(place),
        size,
        reason: None,
    })
}

/// The image of one side.
pub fn image(path: &Path, side: &Side) -> Result<Image> {
    let root = root(path)?;
    match side {
        Side::None => Ok(Image {
            path: None,
            size: 0,
            reason: None,
        }),
        Side::Commit { id, file } => {
            relative_file(file)?;
            let ext = extension(file)?;
            let id = commit_id(&root, id)?;
            blob(&root, &format!("{id}:{file}"), &ext)
        }
        Side::Index { file } => {
            relative_file(file)?;
            blob(&root, &format!(":0:{file}"), &extension(file)?)
        }
        Side::Head { file } => {
            relative_file(file)?;
            blob(&root, &format!("HEAD:{file}"), &extension(file)?)
        }
        Side::Work { file } => {
            relative_file(file)?;
            extension(file)?;
            let place = root.join(file);
            let meta = place
                .symlink_metadata()
                .map_err(|e| fail(format!("{file}: {e}")))?;
            if !meta.is_file() {
                return Err(fail("Not an image file"));
            }
            let large = meta.len() > MAX_IMAGE as u64;
            Ok(Image {
                path: (!large).then_some(place),
                size: meta.len(),
                reason: large
                    .then(|| format!("Images over {} MiB are not shown", MAX_IMAGE / 1024 / 1024)),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::gitgraph::git;
    use super::*;

    #[test]
    fn every_version_of_an_image_is_found_and_others_are_refused() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "Test"],
            &["config", "user.email", "test@example.com"],
            &["config", "core.autocrlf", "false"],
        ] {
            git(p, args).unwrap();
        }
        let name = "a ž.PNG";
        std::fs::write(p.join(name), b"one\0").unwrap();
        std::fs::write(p.join("note.txt"), b"text").unwrap();
        git(p, &["add", "--all"]).unwrap();
        git(p, &["commit", "-qm", "one"]).unwrap();
        let id = git(p, &["rev-parse", "HEAD"]).unwrap().trim().to_owned();
        std::fs::write(p.join(name), b"two!\0").unwrap();
        git(p, &["add", "--all"]).unwrap();
        std::fs::write(p.join(name), b"three!\0").unwrap();
        let file = || name.to_owned();
        let read = |side| {
            let image = image(p, &side).unwrap();
            (std::fs::read(image.path.unwrap()).unwrap(), image.size)
        };
        assert_eq!(
            read(Side::Commit {
                id: id.clone(),
                file: file()
            }),
            (b"one\0".to_vec(), 4)
        );
        assert_eq!(read(Side::Head { file: file() }), (b"one\0".to_vec(), 4));
        assert_eq!(read(Side::Index { file: file() }), (b"two!\0".to_vec(), 5));
        assert_eq!(read(Side::Work { file: file() }), (b"three!\0".to_vec(), 7));
        assert_eq!(image(p, &Side::None).unwrap().path, None);
        for wrong in ["note.txt", "../a.png", "gone.png"] {
            for side in [
                Side::Head { file: wrong.into() },
                Side::Work { file: wrong.into() },
                Side::Index { file: wrong.into() },
            ] {
                assert!(image(p, &side).is_err(), "{wrong}");
            }
        }
        let large = std::fs::File::create(p.join("large.png")).unwrap();
        large.set_len(MAX_IMAGE as u64 + 1).unwrap();
        let found = image(
            p,
            &Side::Work {
                file: "large.png".into(),
            },
        )
        .unwrap();
        assert!(found.path.is_none() && found.reason.is_some());
    }
}
