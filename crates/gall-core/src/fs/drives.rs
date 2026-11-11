//! Drive / mount enumeration for the Drives group and Drives column (S-02, N-02).

use serde::Serialize;
use std::path::Path;
use sysinfo::Disks;

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Drive {
    /// Short label shown in the drives column (e.g. `C:` or `/`, `data`).
    pub short: String,
    /// Volume label / friendly name.
    pub label: String,
    pub mount: String,
    pub fs: String,
    pub total: u64,
    pub available: u64,
    pub removable: bool,
    /// An unlocked encrypted volume that can be locked again (sidebar lock button).
    #[serde(default)]
    pub lockable: bool,
    /// Name of another row that already shows this row's space ("/" or "~"): a mount on
    /// the same filesystem (a btrfs subvolume such as /home or /var/log, a bind mount)
    /// says so and draws no bar of its own, since its numbers are the whole partition's.
    #[serde(default)]
    pub space_shown_by: String,
    /// Backing device (`/dev/nvme0n1p3`), to tell which mounts share one filesystem.
    #[serde(skip)]
    pub device: String,
}

/// Filesystems that never represent user storage.
const PSEUDO_FS: &[&str] = &[
    "tmpfs",
    "devtmpfs",
    "proc",
    "sysfs",
    "cgroup",
    "cgroup2",
    "overlay",
    "squashfs",
    "autofs",
    "debugfs",
    "tracefs",
    "securityfs",
    "pstore",
    "bpf",
    "configfs",
    "fusectl",
    "mqueue",
    "hugetlbfs",
    "ramfs",
    "efivarfs",
    "nsfs",
    "binfmt_misc",
];

fn is_user_visible(mount: &str, fs: &str) -> bool {
    if PSEUDO_FS.contains(&fs) {
        return false;
    }
    if cfg!(unix) {
        // System-internal mount points that clutter the list.
        const HIDDEN_PREFIX: &[&str] = &[
            "/boot",
            "/snap",
            "/var/lib",
            "/sys",
            "/proc",
            "/dev",
            "/run/user",
            "/tmp",
        ];
        if HIDDEN_PREFIX
            .iter()
            .any(|p| mount == *p || mount.starts_with(&format!("{p}/")))
            || super::wsl::hidden_mount(mount)
        {
            return false;
        }
    }
    true
}

fn short_name(mount: &str, label: &str) -> String {
    if cfg!(windows) {
        return mount.trim_end_matches('\\').to_string();
    }
    if mount == "/" {
        return "/".into();
    }
    if !label.is_empty() {
        return label.to_string();
    }
    mount.rsplit('/').next().unwrap_or(mount).to_string()
}

/// Returns visible drives. The home directory is always listed first on Unix
/// because it is where users actually work, followed by `/`, then the rest.
pub fn list_drives() -> Vec<Drive> {
    let disks = match Disks::new_with_refreshed_list() {
        Ok(d) => d,
        Err(e) => {
            log::warn!("cannot list disks: {e}");
            return Vec::new();
        }
    };
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Drive> = Vec::new();
    for d in disks.iter() {
        let mount = d.mount_point().to_string_lossy().into_owned();
        let fs = d.file_system().to_string_lossy().into_owned();
        if !is_user_visible(&mount, &fs) || !seen.insert(mount.clone()) {
            continue;
        }
        let device = d.name().to_string_lossy().into_owned();
        let label = if device.starts_with("/dev/") {
            String::new()
        } else {
            device.clone()
        };
        // Gall inside WSL: /mnt/c is drive C: of Windows.
        let (short, label) = match super::wsl::drvfs_letter(&mount, &fs) {
            Some(letter) => (letter, "Windows".to_string()),
            None => (short_name(&mount, &label), label),
        };
        out.push(Drive {
            short,
            label,
            mount,
            fs,
            total: d.total_space(),
            available: d.available_space(),
            removable: d.is_removable(),
            lockable: false,
            space_shown_by: String::new(),
            device,
        });
    }
    #[cfg(target_os = "linux")]
    for v in fuse_vaults() {
        if seen.insert(v.mount.clone()) {
            out.push(v);
        }
    }
    let lockable = super::volumes::lockable_mounts();
    for d in out.iter_mut() {
        d.lockable = lockable.contains(&d.mount);
    }
    out.sort_by(|a, b| a.mount.cmp(&b.mount));
    #[cfg(unix)]
    if let Some(home) = dirs::home_dir() {
        out = fold_home(out, &home.to_string_lossy());
    }
    #[cfg(unix)]
    share_space(&mut out);
    // WSL distributions (Windows; detected in the background, never waited for).
    out.extend(super::wsl::drives());
    out
}

/// Marks every row whose backing device is already shown by another row: btrfs
/// subvolumes and bind mounts report the whole partition's numbers, so /var/log on the
/// root filesystem would otherwise show the same bar as "/" and read as a second disk.
/// The bar stays on "/" when it is among them, else on the first row ("~" or the
/// shortest mount, the list being sorted).
#[cfg(unix)]
fn share_space(out: &mut [Drive]) {
    for i in 0..out.len() {
        let dev = &out[i].device;
        if dev.is_empty() {
            continue;
        }
        let owner = out
            .iter()
            .position(|d| d.mount == "/" && d.device == *dev)
            .or_else(|| out.iter().position(|d| d.device == *dev))
            .filter(|&o| o != i);
        if let Some(o) = owner {
            out[i].space_shown_by = out[o].short.clone();
        }
    }
}

/// True when `path` is `mount` or lies below it (component-wise: /home2 is not in /home).
#[cfg(unix)]
fn is_under(path: &str, mount: &str) -> bool {
    mount == "/" || path == mount || path.starts_with(&format!("{}/", mount.trim_end_matches('/')))
}

/// Puts a "~ Home" entry first, reporting the space of the filesystem that holds $HOME.
/// That filesystem's own mount (e.g. a separate /home) is dropped since "~" stands for
/// it; "/" and removable drives always stay. "~" carries the device of that filesystem
/// so [`share_space`] leaves its bar to "/" when both are one partition.
#[cfg(unix)]
fn fold_home(mut out: Vec<Drive>, home: &str) -> Vec<Drive> {
    if out.iter().any(|d| d.mount == home) {
        return out;
    }
    let host = out
        .iter()
        .filter(|d| is_under(home, &d.mount))
        .max_by_key(|d| d.mount.len())
        .cloned();
    if let Some(h) = &host {
        if h.mount != "/" && !h.removable {
            out.retain(|d| d.mount != h.mount);
        }
    }
    out.insert(
        0,
        Drive {
            short: "~".into(),
            label: "Home".into(),
            mount: home.to_string(),
            fs: host.as_ref().map(|h| h.fs.clone()).unwrap_or_default(),
            total: host.as_ref().map(|h| h.total).unwrap_or(0),
            available: host.as_ref().map(|h| h.available).unwrap_or(0),
            removable: false,
            lockable: false,
            space_shown_by: String::new(),
            device: host.as_ref().map(|h| h.device.clone()).unwrap_or_default(),
        },
    );
    out
}

/// FUSE filesystems that are system plumbing, not user storage.
#[cfg(target_os = "linux")]
const SYSTEM_FUSE: &[&str] = &[
    "fuse.portal",
    "fuse.gvfsd-fuse",
    "fuse.xdg-document-portal",
    "fuse.flatpak",
    "fuse.lxcfs",
    "fuse.snapfuse",
    "fuse.kio-fuse",
];

/// Encrypted vaults and other FUSE mounts (Cryptomator, gocryptfs, CryFS, Plasma
/// Vault, rclone, sshfs) that sysinfo skips when they live under /run/user.
#[cfg(target_os = "linux")]
fn fuse_vaults() -> Vec<Drive> {
    let Ok(text) = std::fs::read_to_string("/proc/self/mounts") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        let (Some(dev), Some(mount), Some(fs)) = (f.first(), f.get(1), f.get(2)) else {
            continue;
        };
        let mount = mount
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\134", "\\");
        if !fs.starts_with("fuse.")
            || SYSTEM_FUSE.contains(fs)
            || !mount.starts_with("/run/user/")
            || mount.contains("/gvfs")
            || mount.contains("/doc")
        {
            continue;
        }
        let (total, available) = statvfs(&mount).unwrap_or((0, 0));
        let name = mount.rsplit('/').next().unwrap_or(&mount).to_string();
        out.push(Drive {
            short: name.clone(),
            label: if *dev != name {
                dev.to_string()
            } else {
                String::new()
            },
            mount,
            fs: fs.trim_start_matches("fuse.").to_string(),
            total,
            available,
            removable: false,
            lockable: false,
            space_shown_by: String::new(),
            device: dev.to_string(),
        });
    }
    out
}

#[cfg(target_os = "linux")]
fn statvfs(path: &str) -> Option<(u64, u64)> {
    let c = std::ffi::CString::new(path).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid NUL-terminated path and an out-struct of the right type.
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let f = s.f_frsize as u64;
    Some((s.f_blocks as u64 * f, s.f_bavail as u64 * f))
}

/// Unmounts / ejects a removable drive using the platform tool. When that fails, the
/// message names the programs that still have files open on the drive ([`holders`]).
pub fn eject(mount: &str) -> crate::Result<()> {
    // A WSL distribution: "Eject" stops it.
    if cfg!(windows) {
        if let Some(w) = super::wsl::parse_unc(mount) {
            return super::wsl::terminate(&w.distro);
        }
    }
    eject_inner(mount).map_err(|e| {
        let who = holders(mount);
        if who.is_empty() {
            return e;
        }
        crate::CoreError::Other(in_use_message(&who))
    })
}

/// "eject failed: the drive is in use by OneCommander; close its files …".
fn in_use_message(who: &[String]) -> String {
    format!(
        "eject failed: the drive is in use by {}; close {} there and try again",
        who.join(", "),
        if who.len() == 1 {
            "its files and windows"
        } else {
            "their files and windows"
        }
    )
}

/// Names of the programs (of this user) with files or folders open on the drive mounted
/// at `mount`, or whose current folder or program file is there; Gall itself is "Gall".
/// Empty when none are found or the platform cannot tell.
pub fn holders(mount: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        match mount.chars().next() {
            Some(l) if l.is_ascii_alphabetic() && mount[1..].starts_with(':') => {
                win_holders::processes(l)
            }
            _ => Vec::new(),
        }
    }
    #[cfg(target_os = "linux")]
    {
        proc_holders(std::path::Path::new("/proc"), std::path::Path::new(mount))
    }
    #[cfg(target_os = "macos")]
    {
        lsof_holders(mount)
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = mount;
        Vec::new()
    }
}

/// Linux (also testable on any Unix-like layout): looks through `<proc>/<pid>/` the way
/// `fuser -m` does: open files (`fd/*`), the current folder (`cwd`; a terminal left in
/// the drive is the usual "target is busy"), the root and the program file (`exe`).
/// Only processes of this user can be read without root, which covers the usual ones.
#[allow(dead_code)]
fn proc_holders(proc: &std::path::Path, mount: &std::path::Path) -> Vec<String> {
    let me = std::process::id().to_string();
    let mut names: Vec<String> = Vec::new();
    let Ok(rd) = std::fs::read_dir(proc) else {
        return names;
    };
    for e in rd.flatten() {
        let pid = e.file_name().to_string_lossy().into_owned();
        if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let dir = e.path();
        let on_drive = |p: std::path::PathBuf| p.starts_with(mount);
        let mut uses = ["cwd", "root", "exe"]
            .iter()
            .filter_map(|l| std::fs::read_link(dir.join(l)).ok())
            .any(on_drive);
        if !uses {
            if let Ok(fds) = std::fs::read_dir(dir.join("fd")) {
                uses = fds
                    .flatten()
                    .filter_map(|f| std::fs::read_link(f.path()).ok())
                    .any(on_drive);
            }
        }
        if !uses {
            continue;
        }
        let name = if pid == me {
            "Gall".to_string()
        } else {
            match std::fs::read_to_string(dir.join("comm")) {
                Ok(c) if !c.trim().is_empty() => c.trim().to_string(),
                _ => continue,
            }
        };
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// macOS: `lsof +f -- <mount>` lists the processes with files open on that file system.
#[cfg(target_os = "macos")]
fn lsof_holders(mount: &str) -> Vec<String> {
    use std::process::{Command, Stdio};
    let Ok(mut child) = Command::new("lsof")
        .args(["-w", "-F", "pc", "+f", "--", mount])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Vec::new();
    };
    // lsof can take long on a busy system; it only adds detail to a message.
    let t0 = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if t0.elapsed() < std::time::Duration::from_secs(5) => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Vec::new();
            }
        }
    }
    let mut out = String::new();
    if let Some(mut so) = child.stdout.take() {
        use std::io::Read;
        let _ = so.read_to_string(&mut out);
    }
    parse_lsof(&out, std::process::id())
}

/// Process names from `lsof -F pc` output ("p<pid>" then "c<command>" lines); the
/// process `me` is "Gall".
#[allow(dead_code)]
fn parse_lsof(out: &str, me: u32) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut pid = 0u32;
    for line in out.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.trim().parse().unwrap_or(0);
        } else if let Some(c) = line.strip_prefix('c') {
            let name = if pid == me {
                "Gall".to_string()
            } else {
                c.trim().to_string()
            };
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

fn eject_inner(mount: &str) -> crate::Result<()> {
    use std::process::Command;
    #[cfg(windows)]
    return eject_windows(mount);
    #[allow(unreachable_code)]
    let attempts: Vec<Vec<&str>> = if cfg!(target_os = "linux") {
        vec![
            vec!["gio", "mount", "-e", mount],
            vec!["udisksctl", "unmount", "-p", mount],
            vec!["umount", mount],
        ]
    } else if cfg!(target_os = "macos") {
        vec![vec!["diskutil", "eject", mount]]
    } else {
        return Err(crate::CoreError::Other(
            "eject is not supported on this platform yet".into(),
        ));
    };
    let mut first_error = None;
    for cmd in attempts {
        match Command::new(cmd[0]).args(&cmd[1..]).output() {
            Ok(o) if o.status.success() => return Ok(()),
            Ok(o) => {
                first_error
                    .get_or_insert_with(|| String::from_utf8_lossy(&o.stderr).trim().to_string());
            }
            Err(e) => {
                first_error.get_or_insert_with(|| format!("{}: {e}", cmd[0]));
            }
        }
        // A tool can unmount the drive and still fail (powering it off, for example):
        // gone is gone. Not checked before the first attempt has had its chance.
        if !is_mounted(mount) {
            return Ok(());
        }
    }
    // The first tool's message says why (the later ones only fail differently).
    Err(crate::CoreError::Other(format!(
        "eject failed: {}",
        first_error.unwrap_or_default()
    )))
}

/// Windows: ejects the drive with the letter of `mount` ("E:\") like "Safely remove" in
/// Explorer (see [`win_pnp::eject`]): Windows asks the programs that have files open on
/// it to let go, and names the one that refuses. When the drive's device cannot be found
/// (an unusual reader), the volume is locked and ejected directly ([`eject_by_lock`]).
#[cfg(windows)]
fn eject_windows(mount: &str) -> crate::Result<()> {
    let letter = mount
        .chars()
        .next()
        .filter(|c| c.is_ascii_alphabetic() && mount[1..].starts_with(':'))
        .ok_or_else(|| crate::CoreError::Other(format!("eject failed: {mount} is no drive")))?;
    match win_pnp::eject(letter) {
        Ok(()) => Ok(()),
        Err(win_pnp::Failure::Vetoed(why)) => {
            Err(crate::CoreError::Other(format!("eject failed: {why}")))
        }
        Err(win_pnp::Failure::NoDevice) => eject_by_lock(mount),
    }
}

/// The programs (of this user) that have files or folders open on drive `letter`: every
/// open handle in the system is looked at, and file handles on the drive are traced back
/// to their process. Only run after an eject failed.
#[cfg(windows)]
pub mod win_holders {
    use std::ffi::c_void;

    #[repr(C)]
    struct Entry {
        object: usize,
        pid: usize,
        handle: usize,
        access: u32,
        creator_back_trace: u16,
        object_type: u16,
        attributes: u32,
        reserved: u32,
    }

    type Handle = isize;
    #[link(name = "ntdll")]
    extern "system" {
        fn NtQuerySystemInformation(class: u32, buf: *mut c_void, len: u32, ret: *mut u32) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn GetCurrentProcess() -> Handle;
        fn GetCurrentProcessId() -> u32;
        fn DuplicateHandle(
            sp: Handle,
            sh: Handle,
            tp: Handle,
            th: *mut Handle,
            access: u32,
            inherit: i32,
            options: u32,
        ) -> i32;
        fn CloseHandle(h: Handle) -> i32;
        fn GetFileType(h: Handle) -> u32;
        fn GetFinalPathNameByHandleW(h: Handle, buf: *mut u16, len: u32, flags: u32) -> u32;
        fn QueryFullProcessImageNameW(h: Handle, flags: u32, buf: *mut u16, len: *mut u32) -> i32;
    }
    const SYSTEM_EXTENDED_HANDLE_INFORMATION: u32 = 64;
    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
    const PROCESS_DUP_HANDLE: u32 = 0x40;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const DUPLICATE_SAME_ACCESS: u32 = 2;
    const FILE_TYPE_DISK: u32 = 1;

    /// All handles in the system (a header count, then the entries).
    fn snapshot() -> Option<Vec<u8>> {
        let mut len: u32 = 1 << 22;
        for _ in 0..8 {
            let mut buf = vec![0u8; len as usize];
            let mut need = 0u32;
            // SAFETY: a buffer of `len` bytes.
            let st = unsafe {
                NtQuerySystemInformation(
                    SYSTEM_EXTENDED_HANDLE_INFORMATION,
                    buf.as_mut_ptr().cast(),
                    len,
                    &mut need,
                )
            };
            if st == 0 {
                return Some(buf);
            }
            if st != STATUS_INFO_LENGTH_MISMATCH {
                return None;
            }
            len = need.max(len.saturating_mul(2)).saturating_add(1 << 16);
        }
        None
    }

    fn process_name(proc: Handle) -> Option<String> {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: a process handle and a buffer of `len` characters.
        (unsafe { QueryFullProcessImageNameW(proc, 0, buf.as_mut_ptr(), &mut len) } != 0).then(
            || {
                let path = String::from_utf16_lossy(&buf[..len as usize]);
                let file = path.rsplit('\\').next().unwrap_or(&path);
                file.strip_suffix(".exe")
                    .or_else(|| file.strip_suffix(".EXE"))
                    .unwrap_or(file)
                    .to_string()
            },
        )
    }

    /// Names of the processes with a file or folder open on drive `letter`, this one
    /// included (shown as "Gall"); empty when none were found or it cannot be told.
    pub fn processes(letter: char) -> Vec<String> {
        let Some(buf) = snapshot() else {
            return Vec::new();
        };
        let prefix: Vec<u16> = format!(r"\\?\{}:", letter.to_ascii_uppercase())
            .encode_utf16()
            .collect();
        let count = usize::from_ne_bytes(buf[..std::mem::size_of::<usize>()].try_into().unwrap());
        let start = std::mem::size_of::<usize>() * 2;
        let size = std::mem::size_of::<Entry>();
        let count = count.min((buf.len() - start) / size);
        // SAFETY: plain handles of this process.
        let (me, my_pid) = unsafe { (GetCurrentProcess(), GetCurrentProcessId()) };
        let mut procs: std::collections::HashMap<usize, Handle> = Default::default();
        let mut names: Vec<String> = Vec::new();
        for i in 0..count {
            // SAFETY: inside the buffer; the entries are plain data (read unaligned).
            let e: Entry =
                unsafe { std::ptr::read_unaligned(buf.as_ptr().add(start + i * size).cast()) };
            let proc = *procs.entry(e.pid).or_insert_with(|| {
                // SAFETY: opening a process; 0 when not allowed (another user, system).
                unsafe {
                    OpenProcess(
                        PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION,
                        0,
                        e.pid as u32,
                    )
                }
            });
            if proc == 0 {
                continue;
            }
            let mut dup: Handle = 0;
            // SAFETY: duplicating into this process; closed below.
            if unsafe {
                DuplicateHandle(
                    proc,
                    e.handle as Handle,
                    me,
                    &mut dup,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            } == 0
            {
                continue;
            }
            // Disk files only: asking a pipe for its name can block.
            // SAFETY: a handle owned here.
            if unsafe { GetFileType(dup) } == FILE_TYPE_DISK {
                let mut path = [0u16; 600];
                // SAFETY: a buffer of the given length.
                let n = unsafe {
                    GetFinalPathNameByHandleW(dup, path.as_mut_ptr(), path.len() as u32, 0)
                } as usize;
                if n >= prefix.len()
                    && n < path.len()
                    && path[..prefix.len()].eq_ignore_ascii_case_u16(&prefix)
                {
                    let name = if e.pid as u32 == my_pid {
                        Some("Gall".to_string())
                    } else {
                        process_name(proc)
                    };
                    if let Some(name) = name {
                        if !names.contains(&name) {
                            names.push(name);
                        }
                    }
                }
            }
            // SAFETY: the duplicate made above.
            unsafe { CloseHandle(dup) };
        }
        for h in procs.into_values().filter(|&h| h != 0) {
            // SAFETY: process handles opened above.
            unsafe { CloseHandle(h) };
        }
        names
    }

    trait EqIgnoreAsciiCaseU16 {
        fn eq_ignore_ascii_case_u16(&self, other: &[u16]) -> bool;
    }
    impl EqIgnoreAsciiCaseU16 for [u16] {
        fn eq_ignore_ascii_case_u16(&self, other: &[u16]) -> bool {
            let low = |c: u16| {
                if (b'A' as u16..=b'Z' as u16).contains(&c) {
                    c + 32
                } else {
                    c
                }
            };
            self.len() == other.len() && self.iter().zip(other).all(|(&a, &b)| low(a) == low(b))
        }
    }
}

/// PnP ejection, as Explorer's "Safely remove hardware": the volume's disk is found by
/// its device number, and its parent (the USB storage device) is asked to eject. Windows
/// then sends a query-remove to every program registered for it, so well-behaved ones
/// (Explorer, file managers) close their handles first; a program that refuses is named.
#[cfg(windows)]
mod win_pnp {
    use std::ffi::c_void;

    pub enum Failure {
        /// The drive's device was not found: try another way.
        NoDevice,
        /// Windows refused, with the reason in words.
        Vetoed(String),
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid(u32, u16, u16, [u8; 8]);
    const GUID_DEVINTERFACE_DISK: Guid = Guid(
        0x53f5_6307,
        0xb6bf,
        0x11d0,
        [0x94, 0xf2, 0x00, 0xa0, 0xc9, 0x1e, 0xfb, 0x8b],
    );

    #[repr(C)]
    struct DeviceInterfaceData {
        size: u32,
        class: Guid,
        flags: u32,
        reserved: usize,
    }

    #[repr(C)]
    struct DevInfoData {
        size: u32,
        class: Guid,
        dev_inst: u32,
        reserved: usize,
    }

    #[repr(C)]
    #[derive(Default, PartialEq, Eq)]
    struct StorageDeviceNumber {
        device_type: u32,
        device_number: u32,
        partition_number: u32,
    }

    type Handle = isize;
    const INVALID: Handle = -1;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const c_void,
            disposition: u32,
            flags: u32,
            template: Handle,
        ) -> Handle;
        fn DeviceIoControl(
            h: Handle,
            code: u32,
            input: *const c_void,
            input_size: u32,
            output: *mut c_void,
            output_size: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn CloseHandle(h: Handle) -> i32;
    }
    #[link(name = "setupapi")]
    extern "system" {
        fn SetupDiGetClassDevsW(
            class: *const Guid,
            enumerator: *const u16,
            parent: isize,
            flags: u32,
        ) -> Handle;
        fn SetupDiEnumDeviceInterfaces(
            set: Handle,
            info: *const DevInfoData,
            class: *const Guid,
            index: u32,
            data: *mut DeviceInterfaceData,
        ) -> i32;
        fn SetupDiGetDeviceInterfaceDetailW(
            set: Handle,
            data: *const DeviceInterfaceData,
            detail: *mut u8,
            size: u32,
            required: *mut u32,
            info: *mut DevInfoData,
        ) -> i32;
        fn SetupDiDestroyDeviceInfoList(set: Handle) -> i32;
    }
    #[link(name = "cfgmgr32")]
    extern "system" {
        fn CM_Get_Parent(parent: *mut u32, dev_inst: u32, flags: u32) -> u32;
        fn CM_Request_Device_EjectW(
            dev_inst: u32,
            veto_type: *mut i32,
            veto_name: *mut u16,
            name_len: u32,
            flags: u32,
        ) -> u32;
    }

    const IOCTL_STORAGE_GET_DEVICE_NUMBER: u32 = 0x002D_1080;
    const DIGCF_PRESENT: u32 = 0x2;
    const DIGCF_DEVICEINTERFACE: u32 = 0x10;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// Device number of a volume or disk path (opened without access: no rights needed).
    fn device_number(path: &[u16]) -> Option<StorageDeviceNumber> {
        // SAFETY: a NUL-terminated path; the handle is closed below.
        let h = unsafe { CreateFileW(path.as_ptr(), 0, 0x3, std::ptr::null(), 3, 0, 0) };
        if h == INVALID {
            return None;
        }
        let mut n = StorageDeviceNumber::default();
        let mut got = 0u32;
        // SAFETY: an open handle and an output buffer of the given size.
        let ok = unsafe {
            DeviceIoControl(
                h,
                IOCTL_STORAGE_GET_DEVICE_NUMBER,
                std::ptr::null(),
                0,
                &mut n as *mut _ as *mut c_void,
                std::mem::size_of::<StorageDeviceNumber>() as u32,
                &mut got,
                std::ptr::null_mut(),
            )
        } != 0;
        // SAFETY: the handle opened above.
        unsafe { CloseHandle(h) };
        ok.then_some(n)
    }

    /// The device instance of the disk with `number` (same type and number).
    fn disk_instance(want: &StorageDeviceNumber) -> Option<u32> {
        // SAFETY: plain SetupAPI enumeration; the list is destroyed below.
        let set = unsafe {
            SetupDiGetClassDevsW(
                &GUID_DEVINTERFACE_DISK,
                std::ptr::null(),
                0,
                DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
            )
        };
        if set == INVALID {
            return None;
        }
        let mut found = None;
        for index in 0.. {
            let mut iface = DeviceInterfaceData {
                size: std::mem::size_of::<DeviceInterfaceData>() as u32,
                class: GUID_DEVINTERFACE_DISK,
                flags: 0,
                reserved: 0,
            };
            // SAFETY: valid set and output struct with its size set.
            if unsafe {
                SetupDiEnumDeviceInterfaces(
                    set,
                    std::ptr::null(),
                    &GUID_DEVINTERFACE_DISK,
                    index,
                    &mut iface,
                )
            } == 0
            {
                break;
            }
            let mut need = 0u32;
            // SAFETY: a size query (no buffer).
            unsafe {
                SetupDiGetDeviceInterfaceDetailW(
                    set,
                    &iface,
                    std::ptr::null_mut(),
                    0,
                    &mut need,
                    std::ptr::null_mut(),
                )
            };
            if need < 8 {
                continue;
            }
            // SP_DEVICE_INTERFACE_DETAIL_DATA_W: cbSize (8 on 64-bit, 6 on 32-bit), then the path.
            let mut buf = vec![0u32; (need as usize).div_ceil(4)];
            buf[0] = if cfg!(target_pointer_width = "64") {
                8
            } else {
                6
            };
            let mut info = DevInfoData {
                size: std::mem::size_of::<DevInfoData>() as u32,
                class: GUID_DEVINTERFACE_DISK,
                dev_inst: 0,
                reserved: 0,
            };
            // SAFETY: a buffer of `need` bytes with cbSize set, and an info struct.
            let ok = unsafe {
                SetupDiGetDeviceInterfaceDetailW(
                    set,
                    &iface,
                    buf.as_mut_ptr() as *mut u8,
                    need,
                    std::ptr::null_mut(),
                    &mut info,
                )
            } != 0;
            if !ok {
                continue;
            }
            // The NUL-terminated path starts after the 4-byte cbSize.
            let words: &[u16] = unsafe {
                std::slice::from_raw_parts(
                    (buf.as_ptr() as *const u16).add(2),
                    (need as usize - 4) / 2,
                )
            };
            let end = words.iter().position(|&c| c == 0).unwrap_or(words.len());
            let path: Vec<u16> = words[..end].iter().copied().chain(Some(0)).collect();
            if let Some(n) = device_number(&path) {
                if n.device_type == want.device_type && n.device_number == want.device_number {
                    found = Some(info.dev_inst);
                    break;
                }
            }
        }
        // SAFETY: the list created above.
        unsafe { SetupDiDestroyDeviceInfoList(set) };
        found
    }

    /// Words for a PnP veto (PNP_VETO_TYPE) and its name (a program, service or device).
    fn veto_text(kind: i32, name: &str) -> String {
        let program = name.rsplit(['\\', '/']).next().unwrap_or(name);
        match kind {
            3 => format!("“{program}” is using the drive; close it or its windows on the drive"),
            4 => format!("the service “{program}” is using the drive"),
            2 | 5 => "files on the drive are still open; close them and try again".into(),
            12 => "Windows does not allow you to eject this drive".into(),
            _ if !name.is_empty() => format!("Windows refused ({program})"),
            _ => "Windows refused to eject the drive; it may still be in use".into(),
        }
    }

    pub fn eject(letter: char) -> Result<(), Failure> {
        let volume = device_number(&wide(&format!(r"\\.\{letter}:"))).ok_or(Failure::NoDevice)?;
        let disk = disk_instance(&volume).ok_or(Failure::NoDevice)?;
        let mut parent = 0u32;
        // SAFETY: an output for the parent's device instance.
        if unsafe { CM_Get_Parent(&mut parent, disk, 0) } != 0 {
            return Err(Failure::NoDevice);
        }
        // A program that just closed its files may need a moment: a few tries.
        let mut last = String::new();
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            let mut kind = 0i32;
            let mut name = [0u16; 260];
            // SAFETY: outputs for the veto type and a name buffer of its length. With a
            // veto buffer given, Windows shows no dialog of its own.
            let cr = unsafe {
                CM_Request_Device_EjectW(parent, &mut kind, name.as_mut_ptr(), name.len() as u32, 0)
            };
            if cr == 0 && kind == 0 {
                return Ok(());
            }
            let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
            last = veto_text(kind, &String::from_utf16_lossy(&name[..end]));
        }
        Err(Failure::Vetoed(last))
    }
}

/// Windows fallback: ejects the removable drive the way Microsoft documents it for
/// removable media: lock the volume (fails while a program has files open on it),
/// dismount it, allow removal, eject. Needs no admin rights.
#[cfg(windows)]
fn eject_by_lock(mount: &str) -> crate::Result<()> {
    use std::ffi::c_void;
    type Handle = isize;
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const c_void,
            disposition: u32,
            flags: u32,
            template: Handle,
        ) -> Handle;
        fn DeviceIoControl(
            h: Handle,
            code: u32,
            input: *const c_void,
            input_size: u32,
            output: *mut c_void,
            output_size: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn CloseHandle(h: Handle) -> i32;
    }
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;
    const OPEN_EXISTING: u32 = 3;
    const INVALID_HANDLE: Handle = -1;
    const FSCTL_LOCK_VOLUME: u32 = 0x0009_0018;
    const FSCTL_DISMOUNT_VOLUME: u32 = 0x0009_0020;
    const IOCTL_STORAGE_MEDIA_REMOVAL: u32 = 0x002D_4804;
    const IOCTL_STORAGE_EJECT_MEDIA: u32 = 0x002D_4808;

    let fail = |what: &str| {
        crate::CoreError::Other(format!(
            "eject failed: {what} ({})",
            std::io::Error::last_os_error()
        ))
    };
    let letter = mount
        .chars()
        .next()
        .filter(|c| c.is_ascii_alphabetic() && mount[1..].starts_with(':'))
        .ok_or_else(|| crate::CoreError::Other(format!("eject failed: {mount} is no drive")))?;
    let device: Vec<u16> = format!(r"\\.\{letter}:")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // SAFETY: a NUL-terminated device path; the handle is closed below.
    let h = unsafe {
        CreateFileW(
            device.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        )
    };
    if h == INVALID_HANDLE {
        return Err(fail("the drive cannot be opened"));
    }
    let ioctl = |code: u32, input: Option<&u8>| {
        let mut n = 0u32;
        let (ptr, len) = input.map_or((std::ptr::null(), 0), |b| {
            (b as *const u8 as *const c_void, 1)
        });
        // SAFETY: an open handle; the input is a single byte or nothing.
        unsafe {
            DeviceIoControl(
                h,
                code,
                ptr,
                len,
                std::ptr::null_mut(),
                0,
                &mut n,
                std::ptr::null_mut(),
            ) != 0
        }
    };
    let res = (|| {
        // A program that just closed a file may still hold the volume for a moment.
        let locked = (0..10).any(|i| {
            if i > 0 {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            ioctl(FSCTL_LOCK_VOLUME, None)
        });
        if !locked {
            return Err(fail(
                "the drive is in use; close the files and folders open on it",
            ));
        }
        if !ioctl(FSCTL_DISMOUNT_VOLUME, None) {
            return Err(fail("the drive cannot be dismounted"));
        }
        let prevent_removal = 0u8; // PREVENT_MEDIA_REMOVAL { PreventMediaRemoval: FALSE }
        if !ioctl(IOCTL_STORAGE_MEDIA_REMOVAL, Some(&prevent_removal)) {
            return Err(fail("the drive does not allow removal"));
        }
        if !ioctl(IOCTL_STORAGE_EJECT_MEDIA, None) {
            return Err(fail("the drive cannot be ejected"));
        }
        Ok(())
    })();
    // SAFETY: the handle opened above. Closing it also unlocks the volume after a failure.
    unsafe { CloseHandle(h) };
    res
}

/// True while `mount` is a mount point. Linux: listed in /proc/self/mounts. macOS: the
/// folder is on another device than its parent (`/Volumes/X` goes away or becomes a plain
/// folder when unmounted). Windows ejects through its own API and only uses the result.
fn is_mounted(mount: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        match std::fs::read_to_string("/proc/self/mounts") {
            Ok(t) => mount_points(&t).iter().any(|m| m == mount),
            Err(_) => true,
        }
    }
    #[cfg(target_os = "macos")]
    {
        is_mount_point(std::path::Path::new(mount))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = mount;
        true
    }
}

/// Unix: `p` is a mount point (its device differs from its parent's); false when gone.
#[cfg(unix)]
#[allow(dead_code)]
fn is_mount_point(p: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(m) = std::fs::metadata(p) else {
        return false;
    };
    match p.parent().map(std::fs::metadata) {
        Some(Ok(parent)) => m.dev() != parent.dev(),
        // "/" (no parent): always mounted.
        _ => true,
    }
}

/// Mount points in a /proc/self/mounts text (`\040` = space and other octal escapes).
pub fn mount_points(mounts: &str) -> Vec<String> {
    mounts
        .lines()
        .filter_map(|l| l.split(' ').nth(1))
        .map(|p| {
            let b = p.as_bytes();
            let mut out = Vec::with_capacity(b.len());
            let mut i = 0;
            while i < b.len() {
                let octal = b
                    .get(i + 1..i + 4)
                    .filter(|d| d.iter().all(|c| (b'0'..=b'7').contains(c)));
                if let (b'\\', Some(d)) = (b[i], octal) {
                    out.push(
                        d.iter()
                            .fold(0u8, |v, c| v.wrapping_mul(8).wrapping_add(c - b'0')),
                    );
                    i += 4;
                } else {
                    out.push(b[i]);
                    i += 1;
                }
            }
            String::from_utf8_lossy(&out).into_owned()
        })
        .collect()
}

/// Where removable drives get mounted: a missing folder right below one of these is a
/// drive that is not connected, not a deleted folder.
#[cfg(unix)]
fn is_mount_parent(p: &Path) -> bool {
    let s = p.to_string_lossy();
    let parts: Vec<&str> = s.split('/').filter(|x| !x.is_empty()).collect();
    matches!(
        parts.as_slice(),
        ["run", "media", _] | ["media"] | ["media", _] | ["mnt"] | ["Volumes"]
    )
}

/// Mount points listed in /etc/fstab: an unmounted one is an empty folder, so a missing
/// folder in it is on a drive that is not mounted.
#[cfg(target_os = "linux")]
fn fstab_mounts() -> Vec<std::path::PathBuf> {
    std::fs::read_to_string("/etc/fstab")
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_whitespace().nth(1))
        .filter(|m| m.starts_with('/'))
        .map(|m| m.replace("\\040", " ").into())
        .collect()
}

/// True when the folder `path` was deleted or renamed: it is missing while the folder
/// it was in is there. Not when its drive is not connected or mounted (a missing drive
/// letter, a folder under /run/media/<user>, /media, /mnt or /Volumes, an fstab mount
/// point): those folders come back with the drive. History drops the gone ones.
pub fn folder_gone(path: &Path) -> bool {
    // Looking into a stopped WSL distribution would start it.
    if !super::wsl::may_touch(path) {
        return false;
    }
    // Only a clear "not found": no permission or a network error is not "deleted".
    if !matches!(path.try_exists(), Ok(false)) {
        return false;
    }
    #[cfg(target_os = "linux")]
    let fstab = fstab_mounts();
    let mut gone = path;
    for anc in path.ancestors().skip(1) {
        if anc.as_os_str().is_empty() {
            break;
        }
        if anc.try_exists().unwrap_or(true) {
            #[cfg(unix)]
            if is_mount_parent(anc) {
                return false;
            }
            #[cfg(target_os = "linux")]
            if fstab.iter().any(|m| m == anc || m == gone) {
                return false;
            }
            let _ = gone;
            return true;
        }
        gone = anc;
    }
    // Nothing above exists: a drive letter or network share that is not there.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn mount_points_by_device() {
        assert!(is_mount_point(std::path::Path::new("/")));
        let t = tempfile::tempdir().unwrap();
        assert!(!is_mount_point(&t.path().join("gone")));
        std::fs::create_dir(t.path().join("plain")).unwrap();
        assert!(!is_mount_point(&t.path().join("plain")));
    }

    #[test]
    fn lsof_output_is_parsed_and_message_names_programs() {
        let out = "p101\ncbash\np202\ncvim\np303\ncvim\np7\ncgall\n";
        assert_eq!(parse_lsof(out, 7), ["bash", "vim", "Gall"]);
        assert_eq!(
            in_use_message(&["OneCommander".into()]),
            "eject failed: the drive is in use by OneCommander; close its files and windows there and try again"
        );
        assert!(in_use_message(&["bash".into(), "vim".into()]).contains("bash, vim; close their"));
    }

    /// Linux /proc layout (made up): a terminal in the drive, an editor with a file open
    /// on it, a process elsewhere; a similar mount name does not count.
    #[test]
    fn proc_holders_finds_cwd_and_open_files() {
        // Links to paths that need not exist. Windows allows them only with Developer
        // Mode (or admin rights): skipped otherwise.
        #[cfg(unix)]
        let symlink = |to: std::path::PathBuf, at: std::path::PathBuf| {
            std::os::unix::fs::symlink(to, at).map_err(|_| ())
        };
        #[cfg(windows)]
        let symlink = |to: std::path::PathBuf, at: std::path::PathBuf| {
            std::os::windows::fs::symlink_file(to, at).map_err(|_| ())
        };
        let t = tempfile::tempdir().unwrap();
        if symlink(t.path().join("x"), t.path().join("probe")).is_err() {
            eprintln!("symlinks not allowed here: skipped");
            return;
        }
        let proc = t.path().join("proc");
        let mount = t.path().join("media/Stick");
        std::fs::create_dir_all(mount.join("docs")).unwrap();
        let mk = |pid: &str, comm: &str| {
            let d = proc.join(pid);
            std::fs::create_dir_all(d.join("fd")).unwrap();
            std::fs::write(d.join("comm"), format!("{comm}\n")).unwrap();
            d
        };
        let term = mk("100", "bash");
        symlink(mount.join("docs"), term.join("cwd")).unwrap();
        let ed = mk("200", "vim");
        symlink(t.path().join("elsewhere"), ed.join("cwd")).unwrap();
        symlink(mount.join("docs/a.txt"), ed.join("fd/4")).unwrap();
        let other = mk("300", "other");
        symlink(t.path().join("media/Stick2/x"), other.join("fd/3")).unwrap();
        std::fs::create_dir_all(proc.join("self")).unwrap();
        let mut got = proc_holders(&proc, &mount);
        got.sort();
        assert_eq!(got, ["bash", "vim"]);
    }

    /// The real /proc: a file this process holds open on a folder names "Gall".
    #[cfg(target_os = "linux")]
    #[test]
    fn holders_on_linux_names_this_process() {
        let d = tempfile::tempdir().unwrap();
        let f = std::fs::File::create(d.path().join("held.txt")).unwrap();
        assert!(holders(&d.path().to_string_lossy()).contains(&"Gall".to_string()));
        drop(f);
    }

    /// A file held open on a drive names its process ("Gall" for this one).
    #[cfg(windows)]
    #[test]
    fn processes_holding_a_drive_are_named() {
        let d = tempfile::tempdir().unwrap();
        let f = std::fs::File::create(d.path().join("held.txt")).unwrap();
        let letter = d.path().to_string_lossy().chars().next().unwrap();
        let who = win_holders::processes(letter);
        assert!(who.contains(&"Gall".to_string()), "{who:?}");
        drop(f);
        if let Ok(l) = std::env::var("GALL_PROBE_DRIVE") {
            eprintln!(
                "holding {l}: {:?}",
                win_holders::processes(l.chars().next().unwrap())
            );
        }
    }

    /// A drive that is not there is a clear error, not a panic or a hang.
    #[cfg(windows)]
    #[test]
    fn eject_of_a_missing_or_bad_drive_fails_cleanly() {
        // `Path::exists` is not enough: a letter can fail it and still open as
        // `\\.\X:`. Only a letter missing from the logical-drive bitmask is free.
        #[link(name = "kernel32")]
        extern "system" {
            fn GetLogicalDrives() -> u32;
        }
        let used = unsafe { GetLogicalDrives() };
        let free = (b'D'..=b'Z')
            .find(|c| used & (1 << (c - b'A')) == 0)
            .map(|c| format!("{}:\\", c as char))
            .unwrap();
        let e = eject(&free).unwrap_err().to_string();
        assert!(e.contains("cannot be opened"), "{e}");
        assert!(eject("Čudno").is_err());
        assert!(eject("").is_err());
    }

    #[test]
    fn mount_points_are_unescaped() {
        let t = "/dev/sda1 / ext4 rw 0 0\n/dev/sdb1 /run/media/u/My\\040Stick vfat rw 0 0\n";
        assert_eq!(mount_points(t), vec!["/", "/run/media/u/My Stick"]);
    }

    #[test]
    fn pseudo_filesystems_hidden() {
        assert!(!is_user_visible("/sys/fs/cgroup", "cgroup2"));
        assert!(!is_user_visible("/run/user/1000", "tmpfs"));
        assert!(is_user_visible("/", "btrfs"));
        assert!(is_user_visible("/run/media/u/USB", "vfat"));
    }

    #[cfg(unix)]
    fn drive(mount: &str, total: u64) -> Drive {
        Drive {
            short: short_name(mount, ""),
            label: String::new(),
            mount: mount.into(),
            fs: "btrfs".into(),
            total,
            available: total / 2,
            removable: false,
            lockable: false,
            space_shown_by: String::new(),
            device: format!("/dev/{}", mount.trim_start_matches('/')),
        }
    }

    // Home folding is Unix-only (Windows lists drive letters).
    #[cfg(unix)]
    #[test]
    fn separate_home_mount_folds_into_tilde() {
        let mut out = fold_home(
            vec![drive("/", 10), drive("/home", 20), drive("/data", 30)],
            "/home/alex",
        );
        share_space(&mut out);
        let mounts: Vec<_> = out.iter().map(|d| d.short.as_str()).collect();
        assert_eq!(mounts, ["~", "/", "data"]);
        assert_eq!(out[0].total, 20, "~ reports the /home filesystem");
        assert_eq!(out[0].space_shown_by, "", "own partition: ~ keeps its bar");
    }

    #[cfg(unix)]
    #[test]
    fn btrfs_home_subvolume_leaves_space_to_root() {
        let mut home = drive("/home", 10);
        home.device = "/dev/sda3".into();
        let mut root = drive("/", 10);
        root.device = "/dev/sda3".into();
        let mut out = fold_home(vec![root, home], "/home/u");
        share_space(&mut out);
        let mounts: Vec<_> = out.iter().map(|d| d.short.as_str()).collect();
        assert_eq!(mounts, ["~", "/"]);
        assert_eq!(out[0].space_shown_by, "/");
    }

    #[cfg(unix)]
    #[test]
    fn home_on_root_keeps_root() {
        let mut out = fold_home(vec![drive("/", 10)], "/home/u");
        share_space(&mut out);
        let mounts: Vec<_> = out.iter().map(|d| d.short.as_str()).collect();
        assert_eq!(mounts, ["~", "/"]);
        assert_eq!(out[0].total, 10);
        assert_eq!(out[0].space_shown_by, "/");
    }

    #[cfg(unix)]
    #[test]
    fn btrfs_subvolumes_leave_space_to_root() {
        let on = |mount: &str, dev: &str| {
            let mut d = drive(mount, 10);
            d.device = dev.into();
            d
        };
        let mut out = fold_home(
            vec![
                on("/", "/dev/sda3"),
                on("/home", "/dev/sda3"),
                on("/var/log", "/dev/sda3"),
                on("/mnt/data", "/dev/sdb1"),
                on("/mnt/data/sub", "/dev/sdb1"),
            ],
            "/home/u",
        );
        share_space(&mut out);
        let shown: Vec<_> = out
            .iter()
            .map(|d| (d.short.as_str(), d.space_shown_by.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("~", "/"),
                ("/", ""),
                ("log", "/"),
                ("data", ""),
                ("sub", "data")
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn similar_prefix_is_not_home() {
        let out = fold_home(vec![drive("/", 10), drive("/home2", 5)], "/home/u");
        assert!(out.iter().any(|d| d.mount == "/home2"));
        assert_eq!(out[0].total, 10);
    }

    #[test]
    fn deleted_folder_is_gone() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        std::fs::create_dir(&a).unwrap();
        assert!(!folder_gone(&a), "still there");
        assert!(folder_gone(&t.path().join("b")), "deleted");
        assert!(
            folder_gone(&t.path().join("b/c/d")),
            "inside a deleted folder"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unplugged_drive_is_not_gone() {
        assert!(is_mount_parent(Path::new("/run/media/u")));
        assert!(is_mount_parent(Path::new("/media")));
        assert!(is_mount_parent(Path::new("/mnt")));
        assert!(!is_mount_parent(Path::new("/run/media")));
        assert!(!is_mount_parent(Path::new("/home/u")));
        if Path::new("/mnt").is_dir() {
            assert!(!folder_gone(Path::new("/mnt/no-such-drive-x9/folder")));
        }
    }

    #[test]
    fn listing_does_not_panic() {
        let _ = list_drives();
    }
}
