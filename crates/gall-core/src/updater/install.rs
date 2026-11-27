//! Update staging, pre-flight probing, in-place replacement, and overlay activation.

use super::{
    active_overlay_path, atomic_write_json, read_json_opt, staged_dir, staged_meta_path,
    validate_rel_path, validate_version_dir_name, verify, versions_dir, write_startup_notice,
    ActiveOverlay, ArchiveKind, InstallStrategy, PreviousOverlay, StagedUpdate, StartupNotice,
    UpdateCapability, UpdateStage, NEW_SUFFIX, OLD_SUFFIX, PROBE_FLAG, PROBE_PREFIX,
};
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MAX_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_OUTPUT_CAP: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct StagedResult {
    pub version: String,
}

fn copy_bounded<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    total_written: &mut u64,
) -> Result<(), String> {
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Archive read failed: {e}"))?;
        if n == 0 {
            break;
        }
        *total_written = total_written
            .checked_add(n as u64)
            .ok_or_else(|| "Archive size overflow.".to_string())?;
        if *total_written > MAX_EXTRACTED_BYTES {
            return Err("Update archive exceeds maximum permitted unpacked size.".into());
        }
        writer
            .write_all(&buf[..n])
            .map_err(|e| format!("Archive write failed: {e}"))?;
    }
    Ok(())
}

fn set_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)
            .map_err(|e| format!("Cannot inspect {}: {e}", path.display()))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).map_err(|e| {
            format!(
                "Cannot set executable permissions on {}: {e}",
                path.display()
            )
        })?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn extract_tar_gz(bytes: &[u8], dest: &Path, allow_app_symlinks: bool) -> Result<(), String> {
    let gz = flate2::read::GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(gz);
    let mut total_written = 0u64;
    let mut deferred_symlinks: Vec<(PathBuf, PathBuf)> = Vec::new();

    let entries = archive
        .entries()
        .map_err(|e| format!("Invalid tar.gz archive: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("Corrupt tar entry: {e}"))?;
        let raw_path = entry
            .path()
            .map_err(|e| format!("Invalid tar entry path: {e}"))?
            .into_owned();
        if raw_path.as_os_str() == "." || raw_path.as_os_str().is_empty() {
            continue;
        }
        let rel = validate_rel_path(&raw_path)?;
        let out_path = dest.join(&rel);
        let etype = entry.header().entry_type();

        if etype.is_dir() {
            fs::create_dir_all(&out_path)
                .map_err(|e| format!("Cannot create directory {}: {e}", out_path.display()))?;
        } else if etype.is_file() {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Cannot create directory {}: {e}", parent.display()))?;
            }
            let mut out_file = File::create(&out_path)
                .map_err(|e| format!("Cannot create {}: {e}", out_path.display()))?;
            copy_bounded(&mut entry, &mut out_file, &mut total_written)?;
            #[cfg(unix)]
            if let Ok(mode) = entry.header().mode() {
                use std::os::unix::fs::PermissionsExt;
                let sanitized = (mode & 0o755) | 0o600;
                let _ = fs::set_permissions(&out_path, fs::Permissions::from_mode(sanitized));
            }
        } else if etype.is_symlink() {
            if !allow_app_symlinks {
                return Err(format!(
                    "Refusing unexpected symlink in update archive: {}",
                    rel.display()
                ));
            }
            let link_target = entry
                .link_name()
                .map_err(|e| format!("Invalid symlink target: {e}"))?
                .ok_or_else(|| "Symlink entry missing target.".to_string())?
                .into_owned();
            validate_macos_symlink_target(&rel, &link_target)?;
            deferred_symlinks.push((out_path, link_target));
        } else {
            return Err(format!(
                "Refusing unsupported entry type in update archive: {}",
                rel.display()
            ));
        }
    }

    #[cfg(unix)]
    for (link_path, target) in deferred_symlinks {
        if let Some(parent) = link_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create directory {}: {e}", parent.display()))?;
        }
        std::os::unix::fs::symlink(&target, &link_path)
            .map_err(|e| format!("Cannot create bundle symlink {}: {e}", link_path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = deferred_symlinks;

    Ok(())
}

fn validate_macos_symlink_target(link_rel: &Path, target: &Path) -> Result<(), String> {
    if !link_rel.starts_with("Gall.app") {
        return Err(format!(
            "Refusing symlink outside Gall.app: {}",
            link_rel.display()
        ));
    }
    if target.is_absolute() {
        return Err(format!(
            "Refusing absolute symlink in bundle: {}",
            target.display()
        ));
    }
    let mut stack: Vec<std::ffi::OsString> = link_rel
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_os_string()),
            _ => None,
        })
        .collect();
    for comp in target.components() {
        match comp {
            Component::CurDir => {}
            Component::Normal(s) => stack.push(s.to_os_string()),
            Component::ParentDir => {
                if stack.is_empty() {
                    return Err(format!(
                        "Refusing symlink escaping bundle: {} -> {}",
                        link_rel.display(),
                        target.display()
                    ));
                }
                stack.pop();
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "Refusing symlink escaping bundle: {}",
                    target.display()
                ));
            }
        }
    }
    if stack.first().and_then(|s| s.to_str()) != Some("Gall.app") {
        return Err(format!(
            "Refusing symlink escaping Gall.app: {} -> {}",
            link_rel.display(),
            target.display()
        ));
    }
    Ok(())
}

fn extract_zip(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| format!("Invalid zip archive: {e}"))?;
    let mut total_written = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("Corrupt zip entry: {e}"))?;
        let entry_name = entry
            .name()
            .map_err(|e| format!("Invalid zip entry name: {e}"))?
            .into_owned();
        if entry.is_symlink() {
            return Err(format!(
                "Refusing symlink inside Windows update archive: {entry_name}"
            ));
        }
        let raw_rel = Path::new(&entry_name);
        if raw_rel.as_os_str().is_empty() || raw_rel.as_os_str() == "." {
            continue;
        }
        let rel = validate_rel_path(raw_rel)?;
        let out_path = dest.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&out_path)
                .map_err(|e| format!("Cannot create directory {}: {e}", out_path.display()))?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Cannot create directory {}: {e}", parent.display()))?;
            }
            let mut out_file = File::create(&out_path)
                .map_err(|e| format!("Cannot create {}: {e}", out_path.display()))?;
            copy_bounded(&mut entry, &mut out_file, &mut total_written)?;
        }
    }
    Ok(())
}

/// Finds `(binary_rel, sig_rel, bundle_rel)` inside an unpacked stage directory.
fn locate_extracted_payload(
    root: &Path,
    kind: ArchiveKind,
) -> Result<(PathBuf, PathBuf, Option<PathBuf>), String> {
    let find_pair = |bin_name: &str, sig_name: &str| -> Result<(PathBuf, PathBuf), String> {
        let direct_bin = PathBuf::from(bin_name);
        let direct_sig = PathBuf::from(sig_name);
        if root.join(&direct_bin).is_file() && root.join(&direct_sig).is_file() {
            return Ok((direct_bin, direct_sig));
        }
        for entry in fs::read_dir(root)
            .map_err(|e| format!("Cannot inspect extracted folder {}: {e}", root.display()))?
            .flatten()
        {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                let sub = entry.file_name();
                let cand_bin = PathBuf::from(&sub).join(bin_name);
                let cand_sig = PathBuf::from(&sub).join(sig_name);
                if root.join(&cand_bin).is_file() && root.join(&cand_sig).is_file() {
                    return Ok((cand_bin, cand_sig));
                }
            }
        }
        Err(format!(
            "Update archive is missing {bin_name} or {sig_name}."
        ))
    };

    match kind {
        ArchiveKind::LinuxTarGz => {
            let (bin, sig) = find_pair("gall", "gall.sig")?;
            set_executable(&root.join(&bin))?;
            Ok((bin, sig, None))
        }
        ArchiveKind::WindowsZip => {
            let (bin, sig) = find_pair("gall.exe", "gall.exe.sig")?;
            Ok((bin, sig, None))
        }
        ArchiveKind::MacOsTarGz => {
            let bundle = PathBuf::from("Gall.app");
            let sig = PathBuf::from("gall.sig");
            let macos_dir = root.join(&bundle).join("Contents/MacOS");
            if !macos_dir.is_dir() || !root.join(&sig).is_file() {
                return Err("macOS update archive is missing Gall.app or sibling gall.sig.".into());
            }
            let preferred = bundle.join("Contents/MacOS/gall");
            let bin = if root.join(&preferred).is_file() {
                preferred
            } else {
                // Fallback if bundle executable differs slightly (e.g. "Gall").
                let mut found = None;
                for entry in fs::read_dir(&macos_dir)
                    .map_err(|e| format!("Cannot read {}: {e}", macos_dir.display()))?
                    .flatten()
                {
                    if entry.file_type().is_ok_and(|t| t.is_file()) {
                        found = Some(bundle.join("Contents/MacOS").join(entry.file_name()));
                        break;
                    }
                }
                found
                    .ok_or_else(|| "Gall.app/Contents/MacOS contains no executable.".to_string())?
            };
            set_executable(&root.join(&bin))?;
            Ok((bin, sig, Some(bundle)))
        }
        ArchiveKind::LinuxAppImage => {
            let bin = PathBuf::from("Gall.AppImage");
            let sig = PathBuf::from("Gall.AppImage.sig");
            if !root.join(&bin).is_file() || !root.join(&sig).is_file() {
                return Err("AppImage stage is missing payload or signature.".into());
            }
            Ok((bin, sig, None))
        }
    }
}

fn read_capped<R: Read + Send + 'static>(mut reader: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        while out.len() < PROBE_OUTPUT_CAP {
            let Ok(n) = reader.read(&mut buf) else {
                break;
            };
            if n == 0 {
                break;
            }
            let take = (PROBE_OUTPUT_CAP - out.len()).min(n);
            out.extend_from_slice(&buf[..take]);
        }
        out
    })
}

/// Runs `<exe> --gall-update-probe` with a strict timeout to ensure dynamic libraries,
/// executable permissions, and antivirus/loader checks pass before switching versions.
pub fn probe_binary(
    exe_path: &Path,
    expected_version: &str,
    extra_env: &[(&str, &str)],
) -> Result<(), String> {
    let mut cmd = Command::new(exe_path);
    cmd.arg(PROBE_FLAG)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Strip AppImage variables so a staged AppImage probes itself cleanly.
    for key in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
        cmd.env_remove(key);
    }
    #[cfg(debug_assertions)]
    cmd.env_remove("GALL_UPDATER_TEST_VERSION");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Cannot start downloaded update for verification: {e}"))?;
    let stdout_handle = child.stdout.take().map(read_capped);
    let stderr_handle = child.stderr.take().map(read_capped);

    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Downloaded update verification timed out.".into());
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(format!("Failed waiting for update probe: {e}"));
            }
        }
    };

    let stdout_bytes = stdout_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let stderr_bytes = stderr_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    let stderr = String::from_utf8_lossy(&stderr_bytes);

    if !status.success() {
        return Err(format!(
            "Downloaded update failed compatibility check (exit {}): {}",
            status
                .code()
                .map_or_else(|| "signal".into(), |c| c.to_string()),
            stderr.trim()
        ));
    }

    let expected_line = format!("{PROBE_PREFIX}{expected_version}");
    if stdout.lines().any(|line| line.trim() == expected_line) {
        Ok(())
    } else {
        Err(format!(
            "Downloaded update reported unexpected version probe output: {}",
            stdout.trim()
        ))
    }
}

/// Verifies the downloaded archive, unpacks it safely, verifies the inner binary's signature,
/// probes execution, and stores it ready for instant switch under `<updates>/staging/<version>/`.
#[allow(clippy::too_many_arguments)]
pub fn stage_update<F: FnMut(UpdateStage)>(
    bytes: &[u8],
    outer_sig: &str,
    pubkey: &str,
    expected_version: &str,
    cap: &UpdateCapability,
    require_signed_version: bool,
    probe_extra_env: &[(&str, &str)],
    mut on_stage: F,
) -> Result<StagedResult, String> {
    if !matches!(
        cap.strategy,
        InstallStrategy::InPlace | InstallStrategy::Overlay
    ) {
        return Err(cap.reason.clone());
    }
    validate_version_dir_name(expected_version)?;

    on_stage(UpdateStage::Verifying);
    verify::verify_bytes(
        pubkey,
        outer_sig,
        bytes,
        Some(expected_version),
        require_signed_version,
    )?;

    on_stage(UpdateStage::Extracting);
    let stage_root = staged_dir(&cap.updates_dir);
    fs::create_dir_all(&stage_root)
        .map_err(|e| format!("Cannot create staging folder {}: {e}", stage_root.display()))?;

    let tmp_dir = tempfile::Builder::new()
        .prefix(".work-")
        .tempdir_in(&stage_root)
        .map_err(|e| format!("Cannot create temporary staging folder: {e}"))?;

    match cap.archive_kind {
        ArchiveKind::LinuxTarGz => extract_tar_gz(bytes, tmp_dir.path(), false)?,
        ArchiveKind::MacOsTarGz => extract_tar_gz(bytes, tmp_dir.path(), true)?,
        ArchiveKind::WindowsZip => extract_zip(bytes, tmp_dir.path())?,
        ArchiveKind::LinuxAppImage => {
            let bin_path = tmp_dir.path().join("Gall.AppImage");
            let sig_path = tmp_dir.path().join("Gall.AppImage.sig");
            fs::write(&bin_path, bytes)
                .map_err(|e| format!("Cannot write {}: {e}", bin_path.display()))?;
            fs::write(&sig_path, outer_sig.trim())
                .map_err(|e| format!("Cannot write {}: {e}", sig_path.display()))?;
            set_executable(&bin_path)?;
        }
    }

    let (bin_rel, sig_rel, bundle_rel) =
        locate_extracted_payload(tmp_dir.path(), cap.archive_kind)?;
    let bin_path = tmp_dir.path().join(&bin_rel);
    let sig_path = tmp_dir.path().join(&sig_rel);

    on_stage(UpdateStage::Verifying);
    verify::verify_file(
        pubkey,
        &sig_path,
        &bin_path,
        Some(expected_version),
        require_signed_version,
    )?;

    on_stage(UpdateStage::Probing);
    probe_binary(&bin_path, expected_version, probe_extra_env)?;

    let version_dir = stage_root.join(expected_version);
    if version_dir.exists() {
        fs::remove_dir_all(&version_dir).map_err(|e| {
            format!(
                "Cannot replace prior staging {}: {e}",
                version_dir.display()
            )
        })?;
    }
    let tmp_path = tmp_dir.keep();
    fs::rename(&tmp_path, &version_dir).map_err(|e| {
        let _ = fs::remove_dir_all(&tmp_path);
        format!("Cannot finalize staged update: {e}")
    })?;

    // Remove older staged versions while leaving the just-staged version intact.
    if let Ok(entries) = fs::read_dir(&stage_root) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name != std::ffi::OsStr::new(expected_version)
                && name != std::ffi::OsStr::new("staged.json")
                && entry.path().is_dir()
            {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }

    let staged = StagedUpdate {
        version: expected_version.to_string(),
        target_key: cap.target_key.clone(),
        archive_kind: cap.archive_kind,
        binary_rel: bin_rel.to_string_lossy().into_owned(),
        sig_rel: sig_rel.to_string_lossy().into_owned(),
        bundle_rel: bundle_rel.map(|p| p.to_string_lossy().into_owned()),
        is_appimage: cap.archive_kind == ArchiveKind::LinuxAppImage,
    };
    atomic_write_json(&staged_meta_path(&cap.updates_dir), &staged)?;
    Ok(StagedResult {
        version: expected_version.to_string(),
    })
}

/// Returns the currently staged update if it matches `version` and passes signature verification.
pub fn verified_staged_version(
    updates_dir: &Path,
    expected_version: &str,
    target_key: &str,
    pubkey: &str,
    require_signed_version: bool,
) -> Option<StagedUpdate> {
    let staged: StagedUpdate = read_json_opt(&staged_meta_path(updates_dir))?;
    if staged.version != expected_version
        || staged.target_key != target_key
        || validate_version_dir_name(&staged.version).is_err()
    {
        return None;
    }
    let bin_rel = validate_rel_path(Path::new(&staged.binary_rel)).ok()?;
    let sig_rel = validate_rel_path(Path::new(&staged.sig_rel)).ok()?;
    if let Some(ref b) = staged.bundle_rel {
        validate_rel_path(Path::new(b)).ok()?;
    }
    let stage_ver_dir = staged_dir(updates_dir).join(&staged.version);
    let bin = stage_ver_dir.join(&bin_rel);
    let sig = stage_ver_dir.join(&sig_rel);
    if !bin.is_file() || !sig.is_file() {
        return None;
    }
    verify::verify_file(
        pubkey,
        &sig,
        &bin,
        Some(&staged.version),
        require_signed_version,
    )
    .ok()?;
    Some(staged)
}

fn clear_staging(updates_dir: &Path) {
    let dir = staged_dir(updates_dir);
    if dir.exists() {
        let _ = fs::remove_dir_all(dir);
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst)
        .map_err(|e| format!("Cannot create directory {}: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("Cannot read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| format!("Directory iteration error: {e}"))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let ftype = entry
            .file_type()
            .map_err(|e| format!("Cannot inspect {}: {e}", src_path.display()))?;
        if ftype.is_symlink() {
            #[cfg(unix)]
            {
                let target = fs::read_link(&src_path)
                    .map_err(|e| format!("Cannot read link {}: {e}", src_path.display()))?;
                std::os::unix::fs::symlink(&target, &dst_path)
                    .map_err(|e| format!("Cannot copy link {}: {e}", dst_path.display()))?;
            }
        } else if ftype.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("Cannot copy {}: {e}", dst_path.display()))?;
            if let Ok(meta) = fs::metadata(&src_path) {
                let _ = fs::set_permissions(&dst_path, meta.permissions());
            }
        }
    }
    Ok(())
}

fn old_sibling_path(target: &Path) -> PathBuf {
    let mut os = target.as_os_str().to_os_string();
    os.push(OLD_SUFFIX);
    PathBuf::from(os)
}

fn new_sibling_path(target: &Path) -> PathBuf {
    let mut os = target.as_os_str().to_os_string();
    os.push(NEW_SUFFIX);
    PathBuf::from(os)
}

fn remove_path_if_exists(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) => if meta.is_dir() && !meta.file_type().is_symlink() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        }
        .map_err(|e| format!("Cannot remove stale {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Cannot inspect {}: {e}", path.display())),
    }
}

fn swap_same_fs(staged_src: &Path, target: &Path, is_dir: bool) -> Result<(), String> {
    let new_tmp = new_sibling_path(target);
    let old_tmp = old_sibling_path(target);
    remove_path_if_exists(&new_tmp)?;
    remove_path_if_exists(&old_tmp)?;

    if is_dir {
        copy_dir_recursive(staged_src, &new_tmp)?;
    } else {
        fs::copy(staged_src, &new_tmp).map_err(|e| {
            let _ = remove_path_if_exists(&new_tmp);
            format!("Cannot stage replacement file {}: {e}", new_tmp.display())
        })?;
        set_executable(&new_tmp)?;
    }

    if let Err(e) = fs::rename(target, &old_tmp) {
        let _ = remove_path_if_exists(&new_tmp);
        return Err(format!(
            "Cannot move existing {} aside: {e}",
            target.display()
        ));
    }
    if let Err(e) = fs::rename(&new_tmp, target) {
        let _ = fs::rename(&old_tmp, target);
        let _ = remove_path_if_exists(&new_tmp);
        return Err(format!(
            "Cannot move new update into {}: {e}",
            target.display()
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn update_nsis_uninstall_version(exe_dir: &Path, new_version: &str) {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Gall",
        KEY_READ | KEY_WRITE,
    ) else {
        return;
    };
    let Ok(loc): Result<String, _> = key.get_value("InstallLocation") else {
        return;
    };
    let trimmed = loc.trim().trim_matches('"');
    if trimmed.is_empty() {
        return;
    }
    if super::paths_equal(Path::new(trimmed), exe_dir) {
        let _ = key.set_value("DisplayVersion", &new_version);
    }
}

#[cfg(not(windows))]
fn update_nsis_uninstall_version(_exe_dir: &Path, _new_version: &str) {}

/// Applies a verified staged update according to `cap.strategy`. Returns the executable path
/// to launch on restart.
pub fn apply_staged(
    cap: &UpdateCapability,
    from_version: &str,
    pubkey: &str,
    require_signed_version: bool,
) -> Result<(PathBuf, String), String> {
    let staged: StagedUpdate = read_json_opt(&staged_meta_path(&cap.updates_dir))
        .ok_or_else(|| "No staged update is ready to apply.".to_string())?;
    validate_version_dir_name(&staged.version)?;
    let bin_rel = validate_rel_path(Path::new(&staged.binary_rel))?;
    let sig_rel = validate_rel_path(Path::new(&staged.sig_rel))?;
    let bundle_rel = staged
        .bundle_rel
        .as_deref()
        .map(|b| validate_rel_path(Path::new(b)))
        .transpose()?;

    let stage_ver_dir = staged_dir(&cap.updates_dir).join(&staged.version);
    let staged_bin = stage_ver_dir.join(&bin_rel);
    let staged_sig = stage_ver_dir.join(&sig_rel);

    // Re-verify right before switching (TOCTOU guard).
    verify::verify_file(
        pubkey,
        &staged_sig,
        &staged_bin,
        Some(&staged.version),
        require_signed_version,
    )?;

    let launch_exe = match cap.strategy {
        InstallStrategy::InPlace => {
            if let Some(ref bundle_rel) = bundle_rel {
                let target_bundle = cap
                    .target_bundle_path
                    .as_deref()
                    .ok_or_else(|| "Missing target .app bundle path.".to_string())?;
                swap_same_fs(&stage_ver_dir.join(bundle_rel), target_bundle, true)?;
                let rel_inside_bundle = bin_rel
                    .strip_prefix(bundle_rel)
                    .map_err(|_| "Invalid macOS bundle binary path.".to_string())?;
                target_bundle.join(rel_inside_bundle)
            } else {
                swap_same_fs(&staged_bin, &cap.target_exe_path, false)?;
                update_nsis_uninstall_version(&cap.install_dir, &staged.version);
                cap.target_exe_path.clone()
            }
        }
        InstallStrategy::Overlay => {
            let ver_root = versions_dir(&cap.updates_dir);
            fs::create_dir_all(&ver_root)
                .map_err(|e| format!("Cannot create {}: {e}", ver_root.display()))?;
            let dest_ver_dir = ver_root.join(&staged.version);
            if dest_ver_dir.exists() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&dest_ver_dir, fs::Permissions::from_mode(0o700));
                }
                fs::remove_dir_all(&dest_ver_dir)
                    .map_err(|e| format!("Cannot replace {}: {e}", dest_ver_dir.display()))?;
            }
            fs::rename(&stage_ver_dir, &dest_ver_dir)
                .map_err(|e| format!("Cannot activate update directory: {e}"))?;

            harden_overlay_permissions(&dest_ver_dir, &dest_ver_dir.join(&bin_rel));

            let prev_active: Option<ActiveOverlay> =
                read_json_opt(&active_overlay_path(&cap.updates_dir));
            let previous_verified = prev_active.and_then(|a| {
                if a.verified && a.version != staged.version {
                    Some(PreviousOverlay {
                        version: a.version,
                        binary_rel: a.binary_rel,
                        sig_rel: a.sig_rel,
                        is_appimage: a.is_appimage,
                    })
                } else {
                    a.previous_verified.filter(|p| p.version != staged.version)
                }
            });

            let rel_prefix = PathBuf::from("versions").join(&staged.version);
            let next_active = ActiveOverlay {
                version: staged.version.clone(),
                binary_rel: rel_prefix.join(&bin_rel).to_string_lossy().into_owned(),
                sig_rel: rel_prefix.join(&sig_rel).to_string_lossy().into_owned(),
                is_appimage: staged.is_appimage,
                verified: false,
                boot_attempts: 1,
                previous_verified,
            };
            atomic_write_json(&active_overlay_path(&cap.updates_dir), &next_active)?;
            cap.updates_dir.join(&next_active.binary_rel)
        }
        _ => return Err(cap.reason.clone()),
    };

    clear_staging(&cap.updates_dir);
    let _ = write_startup_notice(
        &cap.updates_dir,
        &StartupNotice::Updated {
            version: staged.version.clone(),
            from_version: Some(from_version.to_string()),
        },
    );
    Ok((launch_exe, staged.version))
}

fn harden_overlay_permissions(ver_dir: &Path, bin_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(ver_dir, fs::Permissions::from_mode(0o700));
        let _ = fs::set_permissions(bin_path, fs::Permissions::from_mode(0o500));
    }
    #[cfg(not(unix))]
    {
        let _ = (ver_dir, bin_path);
    }
}

/// Removes active overlay configuration and staging so the base installation launches directly.
pub fn revert_overlay(
    updates_dir: &Path,
    from_version: &str,
    base_version: &str,
) -> Result<(), String> {
    let active_path = active_overlay_path(updates_dir);
    if active_path.exists() {
        fs::remove_file(&active_path)
            .map_err(|e| format!("Cannot remove {}: {e}", active_path.display()))?;
    }
    clear_staging(updates_dir);
    let _ = write_startup_notice(
        updates_dir,
        &StartupNotice::Reverted {
            from_version: from_version.to_string(),
            to_version: base_version.to_string(),
        },
    );
    Ok(())
}

/// Keeps at most the active overlay version and one previously verified version on disk.
pub fn prune_old_versions(updates_dir: &Path, active: &ActiveOverlay) {
    let keep_a = active.version.as_str();
    let keep_b = active
        .previous_verified
        .as_ref()
        .map(|p| p.version.as_str());
    let root = versions_dir(updates_dir);
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if name_str == keep_a || keep_b.is_some_and(|b| b == name_str) {
            continue;
        }
        if entry.path().is_dir() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o700));
            }
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_tar_gz(entries: &[(&str, &[u8], Option<&str>)]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut tar_bytes, flate2::Compression::default());
            let mut builder = tar::Builder::new(enc);
            for (path, data, symlink_target) in entries {
                let mut hdr = tar::Header::new_gnu();
                if let Some(target) = symlink_target {
                    hdr.set_entry_type(tar::EntryType::Symlink);
                    hdr.set_size(0);
                    hdr.set_mode(0o777);
                    builder.append_link(&mut hdr, path, target).unwrap();
                } else {
                    hdr.set_size(data.len() as u64);
                    hdr.set_mode(0o755);
                    hdr.set_cksum();
                    builder.append_data(&mut hdr, path, *data).unwrap();
                }
            }
            builder.into_inner().unwrap().finish().unwrap();
        }
        tar_bytes
    }

    #[test]
    fn rejects_unsafe_paths_and_escaping_symlinks_in_archives() {
        assert!(validate_rel_path(Path::new("../evil")).is_err());
        assert!(validate_rel_path(Path::new("/etc/passwd")).is_err());
        assert!(validate_macos_symlink_target(
            Path::new("Gall.app/Contents/MacOS/link"),
            Path::new("../../../../etc/passwd")
        )
        .is_err());
        assert!(validate_macos_symlink_target(
            Path::new("Gall.app/Contents/MacOS/link"),
            Path::new("/tmp/abs")
        )
        .is_err());
        assert!(validate_macos_symlink_target(
            Path::new("Gall.app/Contents/Frameworks/F.framework/Versions/Current"),
            Path::new("A")
        )
        .is_ok());

        let t = tempfile::tempdir().unwrap();
        let bad_tar = build_tar_gz(&[("Gall.app/Contents/bad", b"", Some("../../../../outside"))]);
        assert!(extract_tar_gz(&bad_tar, t.path(), true).is_err());

        let mut bad_zip = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(Cursor::new(&mut bad_zip));
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("../escape.exe", opts).unwrap();
            zw.write_all(b"evil").unwrap();
            zw.finish().unwrap();
        }
        assert!(extract_zip(&bad_zip, t.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn full_stage_probe_in_place_and_overlay_lifecycle_with_real_minisign() {
        use crate::updater::{
            active_overlay_path, bootstrap, detect_capability_with, read_json_opt, CapabilityProbe,
            InstallKind,
        };
        use crate::util::appdirs::AppDirs;
        use base64::Engine;

        const TEST_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEZGMDQwOTBBNTZGQjMyOUEKUldTYU12dFdDZ2tFLzRnVFQ5Nk05YVNaMlVOWWoxNm9tdXE1U002MG1PblZ5M1JsUmlrNk03ZXQK";
        const TEST_TAR_SIG: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVTYU12dFdDZ2tFL3cyaFY3WEoybDJ6VFZmeXFOTVNlZTZ3OTdMbEVEejU4N3o5aXA4UnorWkd2UWtwVFdKNVJEblluUXU3TFFrWWY1bjJYMWs2VFQvbldWanZ0MDk3RkFvPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1MTczNDc4CWZpbGU6dXBkYXRlLnRhci5negl2ZXJzaW9uOjkuOS45CmsvRyt5cURncDhWRVZqaVJFN0VvcHhPUU5rYVcwTEZjK09VYkptS09DWUd4Q1hBQVRpZ0hTRElGdUZLNXJRQzlIN0ZWdkp2NGVOaTBqZTVtMU1iUUNnPT0K";
        const TEST_TAR_B64: &str = "H4sIAAAAAAAAA+3TTY+iMBgHcM/zKdzZ+wzvxqNvoFVAa1uglwnQiQiFYUUd5dMvZi9zmd2TO5nk+TVpm7bpS9L/Lpayd2dKZzAY3Fp1YCof2z8Ms6eaetfRLLXrK6oyUIxeX7n3xW5OzTE+9Pu9Mj6+5n9Z96/5b+rnj+dkXz032cNrmr31H53RavWyxv549uIv+8Onrjw+fPUlwd3suvw/NfvdPc+4RdyyrE/zb2nGh/x369SutiD//4MITCWtWCscViwc75wEqkyqjbWYezXXzEzM2ZVvx2VaDo+LOc5EiOpuTkY6kmIyPvCwWFLJSERVTdhiyrWjvboOl25VHwPH3qQUYVJmC5eaa15cuKBosS1xI2Z2kZRYxir/RRnnscoi0kZ6otiHJBy1KZGtSwrVrbBCHK7Fuly5gTfHgTCjXJ5JgEw6u7Q0rC88EIf1pv7kHbhOgtu4fUynbxev7fYkaetNU2MS8DpxqMU1u0lGUuMhauNgePLz4uRvDXNSXhjO5RpXbL9pUeiHde223CMaeqOlR6IQGV6emjhHuqeLCrcIkVb6EWOaT+SUK8Ml19L3bZleI7k7RxVryCxzE40hn8qlYOMzn2UxpWruVba1KbC+Jsryq/8EAAAAAAAAAAAAAAAAAAAAAAAAAL6X39rihIwAKAAA";
        let tar_bytes = base64::engine::general_purpose::STANDARD
            .decode(TEST_TAR_B64)
            .unwrap();
        let t = tempfile::tempdir().unwrap();
        let bin_dir = t.path().join("bin");
        let data_dir = t.path().join("data");
        fs::create_dir_all(&bin_dir).unwrap();
        let installed_bin = bin_dir.join("gall");
        fs::write(&installed_bin, b"old-binary").unwrap();

        let dirs = AppDirs::at(&data_dir);
        let probe = CapabilityProbe {
            os: "linux",
            arch: "x86_64",
            pubkey: TEST_PUBKEY.into(),
            current_version: "0.2.2".into(),
            current_exe: installed_bin.clone(),
            dirs: dirs.clone(),
            appimage: None,
            is_flatpak: false,
            is_snap: false,
            is_package_managed: false,
            base_exe_env: None,
            base_version_env: None,
            nsis_user_install_dir: None,
            force_overlay: false,
        };
        let cap_inplace = detect_capability_with(&probe);
        assert_eq!(cap_inplace.strategy, InstallStrategy::InPlace);

        let mut seen_stages = Vec::new();
        let staged = stage_update(
            &tar_bytes,
            TEST_TAR_SIG,
            TEST_PUBKEY,
            "9.9.9",
            &cap_inplace,
            true,
            &[],
            |s| seen_stages.push(s),
        )
        .unwrap();
        assert_eq!(staged.version, "9.9.9");
        assert!(seen_stages.contains(&UpdateStage::Probing));

        // Wrong expected version fails probe/signature.
        assert!(stage_update(
            &tar_bytes,
            TEST_TAR_SIG,
            TEST_PUBKEY,
            "8.8.8",
            &cap_inplace,
            true,
            &[],
            |_| {},
        )
        .is_err());

        // Apply in-place swaps file and leaves `.gall-old` for post-restart cleanup.
        let (launch_path, applied_ver) =
            apply_staged(&cap_inplace, "0.2.2", TEST_PUBKEY, true).unwrap();
        assert_eq!(applied_ver, "9.9.9");
        assert_eq!(launch_path, installed_bin);
        assert!(bin_dir.join("gall.gall-old").exists());
        assert_eq!(
            fs::read(bin_dir.join("gall.gall-old")).unwrap(),
            b"old-binary"
        );

        // Non-writable or store-managed installs refuse stage_update cleanly.
        let mut store_probe = probe.clone();
        store_probe.is_package_managed = true;
        let store_cap = detect_capability_with(&store_probe);
        assert_eq!(store_cap.strategy, InstallStrategy::StoreManaged);
        assert_eq!(store_cap.install_kind, InstallKind::DistroPackage);
        assert_eq!(
            stage_update(
                &tar_bytes,
                TEST_TAR_SIG,
                TEST_PUBKEY,
                "9.9.9",
                &store_cap,
                true,
                &[],
                |_| {},
            )
            .unwrap_err(),
            store_cap.reason
        );

        // Now test overlay lifecycle with force_overlay = true (like /usr/bin/gall).
        let mut overlay_probe = probe.clone();
        overlay_probe.force_overlay = true;
        let cap_overlay = detect_capability_with(&overlay_probe);
        assert_eq!(cap_overlay.strategy, InstallStrategy::Overlay);
        assert_eq!(cap_overlay.install_kind, InstallKind::Local);

        stage_update(
            &tar_bytes,
            TEST_TAR_SIG,
            TEST_PUBKEY,
            "9.9.9",
            &cap_overlay,
            true,
            &[],
            |_| {},
        )
        .unwrap();
        assert!(verified_staged_version(
            &dirs.updates(),
            "9.9.9",
            &cap_overlay.target_key,
            TEST_PUBKEY,
            true
        )
        .is_some());

        // Seed an extra stale version folder so prune_old_versions is exercised.
        let stale = versions_dir(&dirs.updates()).join("0.1.0");
        fs::create_dir_all(&stale).unwrap();
        let (overlay_exe, _) = apply_staged(&cap_overlay, "0.2.2", TEST_PUBKEY, true).unwrap();
        assert!(overlay_exe.starts_with(versions_dir(&dirs.updates())));
        let active: ActiveOverlay = read_json_opt(&active_overlay_path(&dirs.updates())).unwrap();
        assert!(!active.verified);

        bootstrap::mark_overlay_healthy(&dirs, "9.9.9");
        let active: ActiveOverlay = read_json_opt(&active_overlay_path(&dirs.updates())).unwrap();
        assert!(active.verified);
        assert!(!stale.exists(), "stale overlay version must be pruned");

        revert_overlay(&dirs.updates(), "9.9.9", "0.2.2").unwrap();
        assert!(!active_overlay_path(&dirs.updates()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn probe_binary_rejects_bad_exit_and_wrong_output() {
        let t = tempfile::tempdir().unwrap();
        let bad = t.path().join("bad.sh");
        fs::write(&bad, b"#!/bin/sh\necho WRONG\nexit 0\n").unwrap();
        set_executable(&bad).unwrap();
        assert!(probe_binary(&bad, "1.2.3", &[]).is_err());

        let fail = t.path().join("fail.sh");
        fs::write(&fail, b"#!/bin/sh\nexit 1\n").unwrap();
        set_executable(&fail).unwrap();
        assert!(probe_binary(&fail, "1.2.3", &[]).is_err());
    }

    #[test]
    fn stage_update_rejects_traversal_version_even_without_signed_version() {
        use crate::updater::{detect_capability_with, CapabilityProbe};
        use crate::util::appdirs::AppDirs;

        const TEST_PUBKEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
        let t = tempfile::tempdir().unwrap();
        let bin_dir = t.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let installed_bin = bin_dir.join("gall");
        fs::write(&installed_bin, b"old-binary").unwrap();

        let probe = CapabilityProbe {
            os: "linux",
            arch: "x86_64",
            pubkey: TEST_PUBKEY.into(),
            current_version: "0.2.2".into(),
            current_exe: installed_bin,
            dirs: AppDirs::at(&t.path().join("data")),
            appimage: Some(bin_dir.join("Gall.AppImage")),
            is_flatpak: false,
            is_snap: false,
            is_package_managed: false,
            base_exe_env: None,
            base_version_env: None,
            nsis_user_install_dir: None,
            force_overlay: false,
        };
        let cap = detect_capability_with(&probe);
        for bad_ver in ["../../evil", "../1.0.0", "1.0.0/sub", "not-semver"] {
            let err = stage_update(
                b"payload",
                "sig",
                TEST_PUBKEY,
                bad_ver,
                &cap,
                false,
                &[],
                |_| {},
            )
            .unwrap_err();
            assert!(
                err.contains("version") || err.contains("Invalid"),
                "unexpected error for {bad_ver}: {err}"
            );
        }
    }
}
