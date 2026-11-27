//! Early-startup update probing, PID synchronization, old-binary cleanup, and overlay handoff.

use super::{
    active_overlay_path, atomic_write_json, macos_bundle_root, path_starts_with, read_json_opt,
    validate_rel_path, validate_version_dir_name, verify, versions_dir, write_startup_notice,
    ActiveOverlay, StartupNotice, BASE_EXE_ENV, BASE_VERSION_ENV, NO_OVERLAY_ENV, OLD_SUFFIX,
    PROBE_FLAG, PROBE_PREFIX, WAIT_PID_FLAG,
};
use crate::util::appdirs::AppDirs;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

const WAIT_PID_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_OVERLAY_BOOT_ATTEMPTS: u32 = 2;

/// Handles `--gall-update-probe` before any GUI, D-Bus, or single-instance setup.
pub fn handle_probe_arg(raw_args: &[OsString], current_version: &str) -> Option<i32> {
    if matches!(raw_args, [_, probe] if probe == PROBE_FLAG) {
        println!("{PROBE_PREFIX}{current_version}");
        let _ = std::io::stdout().flush();
        Some(0)
    } else {
        None
    }
}

/// Removes internal updater flags (`--gall-wait-pid <pid>`, `--gall-update-probe`) and
/// returns the user-facing arguments (excluding `argv[0]`) to forward on handoff.
pub fn strip_internal_args(raw_args: &[OsString]) -> Vec<OsString> {
    let mut out = Vec::new();
    let mut it = raw_args.iter().skip(1);
    while let Some(arg) = it.next() {
        if arg == "--" {
            out.push(arg.clone());
            out.extend(it.cloned());
            break;
        }
        if arg == WAIT_PID_FLAG {
            let _ = it.next();
            continue;
        }
        if arg == PROBE_FLAG || arg.to_string_lossy().starts_with("--gall-wait-pid=") {
            continue;
        }
        out.push(arg.clone());
    }
    out
}

fn parse_wait_pid(raw_args: &[OsString]) -> Option<u32> {
    let mut it = raw_args.iter().skip(1);
    while let Some(arg) = it.next() {
        if arg == "--" {
            break;
        }
        if arg == WAIT_PID_FLAG {
            return it.next().and_then(|v| v.to_str()?.parse::<u32>().ok());
        }
        if let Some(rest) = arg
            .to_str()
            .and_then(|s| s.strip_prefix("--gall-wait-pid="))
        {
            return rest.parse::<u32>().ok();
        }
    }
    None
}

#[cfg(windows)]
fn wait_for_pid_exit(pid: u32) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    // SAFETY: Standard Win32 synchronization handle wait; handle is closed immediately after.
    unsafe {
        if let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(handle, WAIT_PID_TIMEOUT.as_millis() as u32);
            let _ = CloseHandle(handle);
        }
    }
}

#[cfg(unix)]
fn wait_for_pid_exit(pid: u32) {
    let Ok(pid_t) = libc::pid_t::try_from(pid) else {
        return;
    };
    let deadline = Instant::now() + WAIT_PID_TIMEOUT;
    loop {
        // SAFETY: kill(pid, 0) checks process existence without sending a signal.
        let rc = unsafe { libc::kill(pid_t, 0) };
        if rc == -1 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::ESRCH) {
                break;
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(not(any(unix, windows)))]
fn wait_for_pid_exit(_pid: u32) {}

fn remove_old_sibling(target: &Path) {
    let mut os = target.as_os_str().to_os_string();
    os.push(OLD_SUFFIX);
    let old = PathBuf::from(os);
    // Retry briefly on Windows in case AV scanner or handle teardown lags slightly after exit.
    let attempts = if cfg!(windows) { 10 } else { 1 };
    for i in 0..attempts {
        match fs::symlink_metadata(&old) {
            Ok(meta) => {
                let res = if meta.is_dir() && !meta.file_type().is_symlink() {
                    fs::remove_dir_all(&old)
                } else {
                    fs::remove_file(&old)
                };
                if res.is_ok() || i + 1 == attempts {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }
}

/// If launched with `--gall-wait-pid <pid>`, waits for the exiting instance to finish before
/// single-instance initialization and removes deterministic `.gall-old` siblings beside the exe.
pub fn handle_wait_and_cleanup(
    raw_args: &[OsString],
    current_exe: Option<&Path>,
    appimage: Option<&Path>,
) {
    if let Some(pid) = parse_wait_pid(raw_args) {
        if pid > 0 && pid != std::process::id() {
            wait_for_pid_exit(pid);
        }
    }
    if let Some(exe) = current_exe {
        remove_old_sibling(exe);
        if let Some(bundle) = macos_bundle_root(exe) {
            remove_old_sibling(&bundle);
        }
    }
    if let Some(ai) = appimage {
        remove_old_sibling(ai);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoffDecision {
    ContinueBase,
    LaunchOverlay {
        exe_path: PathBuf,
        version: String,
        is_appimage: bool,
        base_exe: PathBuf,
        base_version: String,
    },
}

fn cleanup_obsolete_overlay(updates_dir: &Path) {
    let _ = fs::remove_file(active_overlay_path(updates_dir));
    let ver_dir = versions_dir(updates_dir);
    if ver_dir.is_dir() {
        #[cfg(unix)]
        if let Ok(entries) = fs::read_dir(&ver_dir) {
            use std::os::unix::fs::PermissionsExt;
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let _ = fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o700));
                }
            }
        }
        let _ = fs::remove_dir_all(ver_dir);
    }
}

fn rollback_from_active(
    updates_dir: &Path,
    pubkey: &str,
    base_version: &str,
    base_exe: &Path,
    failed_active: &ActiveOverlay,
    reason: &str,
) -> HandoffDecision {
    let active_path = active_overlay_path(updates_dir);
    let _ = fs::remove_file(&active_path);

    if let Some(prev) = &failed_active.previous_verified {
        let base_sv = semver::Version::parse(base_version.trim_start_matches('v')).ok();
        let prev_sv = semver::Version::parse(prev.version.trim_start_matches('v')).ok();
        let newer_than_base = match (prev_sv, base_sv) {
            (Some(p), Some(b)) => p > b,
            _ => false,
        };
        if newer_than_base && validate_version_dir_name(&prev.version).is_ok() {
            if let (Ok(bin_rel), Ok(sig_rel)) = (
                validate_rel_path(Path::new(&prev.binary_rel)),
                validate_rel_path(Path::new(&prev.sig_rel)),
            ) {
                let bin = updates_dir.join(&bin_rel);
                let sig = updates_dir.join(&sig_rel);
                if bin.is_file()
                    && sig.is_file()
                    && verify::verify_file(
                        pubkey,
                        &sig,
                        &bin,
                        Some(&prev.version),
                        !prev.is_appimage,
                    )
                    .is_ok()
                {
                    let restored = ActiveOverlay {
                        version: prev.version.clone(),
                        binary_rel: bin_rel.to_string_lossy().into_owned(),
                        sig_rel: sig_rel.to_string_lossy().into_owned(),
                        is_appimage: prev.is_appimage,
                        verified: true,
                        boot_attempts: 0,
                        previous_verified: None,
                    };
                    if atomic_write_json(&active_path, &restored).is_ok() {
                        let _ = write_startup_notice(
                            updates_dir,
                            &StartupNotice::RolledBack {
                                failed_version: failed_active.version.clone(),
                                restored_version: prev.version.clone(),
                                reason: reason.to_string(),
                            },
                        );
                        return HandoffDecision::LaunchOverlay {
                            exe_path: bin,
                            version: prev.version.clone(),
                            is_appimage: prev.is_appimage,
                            base_exe: base_exe.to_path_buf(),
                            base_version: base_version.to_string(),
                        };
                    }
                }
            }
        }
    }

    let _ = write_startup_notice(
        updates_dir,
        &StartupNotice::RolledBack {
            failed_version: failed_active.version.clone(),
            restored_version: base_version.to_string(),
            reason: reason.to_string(),
        },
    );
    HandoffDecision::ContinueBase
}

/// Pure decision function for startup overlay handoff (separated from `exec`/`spawn` for unit tests).
pub fn prepare_handoff(
    pubkey: &str,
    current_version: &str,
    dirs: &AppDirs,
    current_exe: Option<&Path>,
    appimage: Option<&Path>,
    no_overlay: bool,
) -> HandoffDecision {
    if pubkey.trim().is_empty() || dirs.portable || no_overlay {
        return HandoffDecision::ContinueBase;
    }
    let Some(base_exe) = appimage.or(current_exe) else {
        return HandoffDecision::ContinueBase;
    };
    let updates_dir = dirs.updates();
    let versions_root = versions_dir(&updates_dir);
    if path_starts_with(base_exe, &versions_root) {
        return HandoffDecision::ContinueBase;
    }

    let active_path = active_overlay_path(&updates_dir);
    let Some(mut active): Option<ActiveOverlay> = read_json_opt(&active_path) else {
        return HandoffDecision::ContinueBase;
    };

    if validate_version_dir_name(&active.version).is_err() {
        let _ = fs::remove_file(&active_path);
        return HandoffDecision::ContinueBase;
    }
    let (Ok(active_sv), Ok(base_sv)) = (
        semver::Version::parse(active.version.trim_start_matches('v')),
        semver::Version::parse(current_version.trim_start_matches('v')),
    ) else {
        let _ = fs::remove_file(&active_path);
        return HandoffDecision::ContinueBase;
    };

    // System package or installer caught up or surpassed the overlay: retire old overlays.
    if active_sv <= base_sv {
        cleanup_obsolete_overlay(&updates_dir);
        return HandoffDecision::ContinueBase;
    }

    if !active.verified && active.boot_attempts >= MAX_OVERLAY_BOOT_ATTEMPTS {
        return rollback_from_active(
            &updates_dir,
            pubkey,
            current_version,
            base_exe,
            &active,
            "The updated version closed unexpectedly before finishing startup.",
        );
    }

    let (Ok(bin_rel), Ok(sig_rel)) = (
        validate_rel_path(Path::new(&active.binary_rel)),
        validate_rel_path(Path::new(&active.sig_rel)),
    ) else {
        return rollback_from_active(
            &updates_dir,
            pubkey,
            current_version,
            base_exe,
            &active,
            "Overlay update metadata contains an invalid relative path.",
        );
    };

    let bin_path = updates_dir.join(&bin_rel);
    let sig_path = updates_dir.join(&sig_rel);
    if !bin_path.is_file() || !sig_path.is_file() {
        return rollback_from_active(
            &updates_dir,
            pubkey,
            current_version,
            base_exe,
            &active,
            "Updated application binary or signature is missing.",
        );
    }
    if let Err(err) = verify::verify_file(
        pubkey,
        &sig_path,
        &bin_path,
        Some(&active.version),
        !active.is_appimage,
    ) {
        return rollback_from_active(
            &updates_dir,
            pubkey,
            current_version,
            base_exe,
            &active,
            &err,
        );
    }

    if !active.verified {
        active.boot_attempts = active.boot_attempts.saturating_add(1);
        if let Err(err) = atomic_write_json(&active_path, &active) {
            eprintln!("gall updater: cannot persist boot attempt counter: {err}");
        }
    }

    HandoffDecision::LaunchOverlay {
        exe_path: bin_path,
        version: active.version,
        is_appimage: active.is_appimage,
        base_exe: base_exe.to_path_buf(),
        base_version: current_version.to_string(),
    }
}

/// Checks for a newer verified user-space overlay and hands off execution before Tauri initializes.
/// Returns `Ok(())` to continue running the base binary, or does not return on successful handoff.
pub fn maybe_handoff_to_overlay(
    pubkey: &str,
    current_version: &str,
    dirs: &AppDirs,
    current_exe: Option<&Path>,
    appimage: Option<&Path>,
    raw_args: &[OsString],
) {
    if super::is_package_managed() {
        return;
    }
    let no_overlay = std::env::var_os(NO_OVERLAY_ENV).is_some_and(|v| !v.is_empty());
    let decision = prepare_handoff(
        pubkey,
        current_version,
        dirs,
        current_exe,
        appimage,
        no_overlay,
    );
    let HandoffDecision::LaunchOverlay {
        exe_path,
        version,
        is_appimage,
        base_exe,
        base_version,
    } = decision
    else {
        return;
    };

    let user_args = strip_internal_args(raw_args);
    let mut cmd = Command::new(&exe_path);
    #[cfg(windows)]
    cmd.arg(WAIT_PID_FLAG).arg(std::process::id().to_string());
    cmd.args(&user_args)
        .env(BASE_EXE_ENV, &base_exe)
        .env(BASE_VERSION_ENV, &base_version);
    if is_appimage {
        for key in ["APPIMAGE", "APPDIR", "ARGV0", "OWD"] {
            cmd.env_remove(key);
        }
    }
    #[cfg(debug_assertions)]
    cmd.env_remove("GALL_UPDATER_TEST_VERSION");

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        eprintln!(
            "gall updater: failed to exec overlay {version} ({}): {err}",
            exe_path.display()
        );
        // Roll back if the overlay binary cannot be exec'd at all so base still starts.
        if let Some(active) = read_json_opt::<ActiveOverlay>(&active_overlay_path(&dirs.updates()))
        {
            let _ = rollback_from_active(
                &dirs.updates(),
                pubkey,
                current_version,
                &base_exe,
                &active,
                &format!("Cannot launch updated executable: {err}"),
            );
        }
    }

    #[cfg(windows)]
    {
        match cmd.spawn() {
            Ok(_) => std::process::exit(0),
            Err(err) => {
                eprintln!(
                    "gall updater: failed to launch overlay {version} ({}): {err}",
                    exe_path.display()
                );
                if let Some(active) =
                    read_json_opt::<ActiveOverlay>(&active_overlay_path(&dirs.updates()))
                {
                    let _ = rollback_from_active(
                        &dirs.updates(),
                        pubkey,
                        current_version,
                        &base_exe,
                        &active,
                        &format!("Cannot launch updated executable: {err}"),
                    );
                }
            }
        }
    }
}

/// Marks the running overlay as healthy once `window_ready` fires and prunes older versions.
pub fn mark_overlay_healthy(dirs: &AppDirs, running_version: &str) {
    let updates_dir = dirs.updates();
    let active_path = active_overlay_path(&updates_dir);
    let Some(mut active): Option<ActiveOverlay> = read_json_opt(&active_path) else {
        return;
    };
    if active.version != running_version {
        return;
    }
    if !active.verified || active.boot_attempts != 0 {
        active.verified = true;
        active.boot_attempts = 0;
        let _ = atomic_write_json(&active_path, &active);
    }
    super::install::prune_old_versions(&updates_dir, &active);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updater::{take_startup_notice, PreviousOverlay};

    const PUBKEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const SIG_TEST: &str = "untrusted comment: signature from minisign secret key\n\
RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\n\
trusted comment: timestamp:1556193335\tfile:test\n\
y/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==";

    #[test]
    fn strip_and_parse_internal_args() {
        let raw = vec![
            OsString::from("gall"),
            OsString::from("--gall-wait-pid"),
            OsString::from("4242"),
            OsString::from("/tmp/work"),
            OsString::from("--gall-update-probe"),
            OsString::from("--newtab"),
        ];
        assert_eq!(parse_wait_pid(&raw), Some(4242));
        assert_eq!(
            strip_internal_args(&raw),
            vec![OsString::from("/tmp/work"), OsString::from("--newtab")]
        );
    }

    #[test]
    fn literal_compare_paths_are_not_updater_switches() {
        let raw = ["gall", "diff", "--wait", "--", PROBE_FLAG, WAIT_PID_FLAG].map(OsString::from);
        assert!(handle_probe_arg(&raw, "test").is_none());
        assert_eq!(parse_wait_pid(&raw), None);
        assert_eq!(strip_internal_args(&raw), raw[1..]);
    }

    #[test]
    fn handoff_clears_obsolete_overlay_when_base_is_newer_or_equal() {
        let t = tempfile::tempdir().unwrap();
        let dirs = AppDirs::at(t.path());
        let updates = dirs.updates();
        let ver_dir = updates.join("versions/0.2.0");
        fs::create_dir_all(&ver_dir).unwrap();
        fs::write(ver_dir.join("gall"), b"test").unwrap();
        fs::write(ver_dir.join("gall.sig"), SIG_TEST).unwrap();
        atomic_write_json(
            &active_overlay_path(&updates),
            &ActiveOverlay {
                version: "0.2.0".into(),
                binary_rel: "versions/0.2.0/gall".into(),
                sig_rel: "versions/0.2.0/gall.sig".into(),
                is_appimage: false,
                verified: true,
                boot_attempts: 0,
                previous_verified: None,
            },
        )
        .unwrap();

        let base = t.path().join("usr/bin/gall");
        let decision = prepare_handoff(PUBKEY, "0.3.0", &dirs, Some(&base), None, false);
        assert_eq!(decision, HandoffDecision::ContinueBase);
        assert!(!active_overlay_path(&updates).exists());
        assert!(!ver_dir.exists());
    }

    #[test]
    fn handoff_verifies_increments_boot_attempts_and_rolls_back_on_crash_loop_or_tamper() {
        let t = tempfile::tempdir().unwrap();
        let dirs = AppDirs::at(t.path());
        let updates = dirs.updates();

        let prev_dir = updates.join("versions/0.3.0");
        let bad_dir = updates.join("versions/0.4.0");
        fs::create_dir_all(&prev_dir).unwrap();
        fs::create_dir_all(&bad_dir).unwrap();
        // Use AppImage flag here so SIG_TEST (which lacks version: tab field) tests real crypto!
        fs::write(prev_dir.join("Gall.AppImage"), b"test").unwrap();
        fs::write(prev_dir.join("Gall.AppImage.sig"), SIG_TEST).unwrap();
        fs::write(bad_dir.join("Gall.AppImage"), b"test").unwrap();
        fs::write(bad_dir.join("Gall.AppImage.sig"), SIG_TEST).unwrap();

        atomic_write_json(
            &active_overlay_path(&updates),
            &ActiveOverlay {
                version: "0.4.0".into(),
                binary_rel: "versions/0.4.0/Gall.AppImage".into(),
                sig_rel: "versions/0.4.0/Gall.AppImage.sig".into(),
                is_appimage: true,
                verified: false,
                boot_attempts: 0,
                previous_verified: Some(PreviousOverlay {
                    version: "0.3.0".into(),
                    binary_rel: "versions/0.3.0/Gall.AppImage".into(),
                    sig_rel: "versions/0.3.0/Gall.AppImage.sig".into(),
                    is_appimage: true,
                }),
            },
        )
        .unwrap();

        let base = t.path().join("opt/Gall.AppImage");
        let d1 = prepare_handoff(PUBKEY, "0.2.0", &dirs, Some(&base), Some(&base), false);
        assert!(
            matches!(d1, HandoffDecision::LaunchOverlay { ref version, .. } if version == "0.4.0")
        );
        let active: ActiveOverlay = read_json_opt(&active_overlay_path(&updates)).unwrap();
        assert_eq!(active.boot_attempts, 1);

        let d2 = prepare_handoff(PUBKEY, "0.2.0", &dirs, Some(&base), Some(&base), false);
        assert!(
            matches!(d2, HandoffDecision::LaunchOverlay { ref version, .. } if version == "0.4.0")
        );
        let active: ActiveOverlay = read_json_opt(&active_overlay_path(&updates)).unwrap();
        assert_eq!(active.boot_attempts, 2);

        // Third startup while still unverified triggers automatic rollback to 0.3.0!
        let d3 = prepare_handoff(PUBKEY, "0.2.0", &dirs, Some(&base), Some(&base), false);
        assert!(
            matches!(d3, HandoffDecision::LaunchOverlay { ref version, .. } if version == "0.3.0")
        );
        let notice = take_startup_notice(&updates);
        assert!(matches!(
            notice,
            Some(StartupNotice::RolledBack {
                failed_version,
                restored_version,
                ..
            }) if failed_version == "0.4.0" && restored_version == "0.3.0"
        ));

        // Tampering the remaining 0.3.0 binary fails signature verification and rolls back to base.
        fs::write(prev_dir.join("Gall.AppImage"), b"TAMPERED").unwrap();
        let d4 = prepare_handoff(PUBKEY, "0.2.0", &dirs, Some(&base), Some(&base), false);
        assert_eq!(d4, HandoffDecision::ContinueBase);
        let notice = take_startup_notice(&updates);
        assert!(matches!(
            notice,
            Some(StartupNotice::RolledBack {
                failed_version,
                restored_version,
                ..
            }) if failed_version == "0.3.0" && restored_version == "0.2.0"
        ));
    }

    #[test]
    fn wait_and_cleanup_removes_only_deterministic_old_siblings() {
        let t = tempfile::tempdir().unwrap();
        let exe = t.path().join("gall");
        let old = t.path().join("gall.gall-old");
        let innocent = t.path().join("keep.txt");
        fs::write(&exe, b"new").unwrap();
        fs::write(&old, b"old").unwrap();
        fs::write(&innocent, b"keep").unwrap();

        handle_wait_and_cleanup(
            &[
                OsString::from("gall"),
                OsString::from("--gall-wait-pid"),
                OsString::from("999999999"),
            ],
            Some(&exe),
            None,
        );
        assert!(!old.exists());
        assert!(exe.exists());
        assert!(innocent.exists());
    }
}
