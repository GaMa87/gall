//! Volumes that are not mounted yet: plain partitions to mount and encrypted ones
//! to unlock (Linux: LUKS and BitLocker through udisks; Windows: BitLocker drives).
//! The sidebar keeps them under "Not mounted" until they are mounted.

use crate::error::{CoreError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    /// What to mount or unlock: a device (`/dev/sdb1`) or a drive (`E:\`).
    pub id: String,
    /// Volume label, else the device name.
    pub label: String,
    pub size: u64,
    pub fs: String,
    /// Encrypted and locked: needs a passphrase (or, on Windows, the system dialog).
    pub locked: bool,
    pub removable: bool,
}

/// One row of `lsblk -J -b -l` (flat list; `pkname` links a node to its parent).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BlockDev {
    pub path: String,
    pub kname: String,
    pub pkname: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub fstype: Option<String>,
    pub label: Option<String>,
    pub parttype: Option<String>,
    pub size: Option<u64>,
    pub mountpoint: Option<String>,
    pub rm: Option<serde_json::Value>,
    pub hotplug: Option<serde_json::Value>,
    pub uuid: Option<String>,
}

#[derive(Deserialize)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct Lsblk {
    blockdevices: Vec<BlockDev>,
}

fn truthy(v: &Option<serde_json::Value>) -> bool {
    // Old lsblk prints "1"/"0" strings, new ones booleans.
    matches!(v, Some(serde_json::Value::Bool(true)))
        || matches!(v, Some(serde_json::Value::String(s)) if s == "1")
}

const ENCRYPTED: &[&str] = &["crypto_LUKS", "BitLocker"];
/// Partition types that are never user storage: EFI, Microsoft reserved, Windows
/// recovery, BIOS boot (GPT GUIDs), and the MBR EFI / recovery types.
const SYSTEM_PARTTYPES: &[&str] = &[
    "c12a7328-f81f-11d2-ba4b-00a0c93ec93b",
    "e3c9e316-0b5c-4db8-817d-f92df00215ae",
    "de94bba4-06d1-4d40-a16a-bfd50179d6ac",
    "21686148-6449-6e6f-744e-656564454649",
    "0xef",
    "0x27",
];
/// Filesystem signatures that are not mountable user filesystems.
const NOT_MOUNTABLE: &[&str] = &[
    "swap",
    "LVM2_member",
    "linux_raid_member",
    "zfs_member",
    "bcache",
    "ceph",
    "drbd",
];

/// The volumes in an `lsblk` listing that are not mounted (pure, for tests).
pub fn unmounted_from(devs: &[BlockDev]) -> Vec<Volume> {
    let has_child = |d: &BlockDev| {
        devs.iter()
            .any(|c| c.pkname.as_deref() == Some(d.kname.as_str()))
    };
    // Removable/hotplug is a property of the whole disk; walk up to it.
    let removable = |d: &BlockDev| {
        let mut cur = Some(d);
        while let Some(n) = cur {
            if truthy(&n.rm) || truthy(&n.hotplug) {
                return true;
            }
            cur = n
                .pkname
                .as_deref()
                .and_then(|p| devs.iter().find(|x| x.kname == p));
        }
        false
    };
    let mut out = Vec::new();
    for d in devs {
        // Loop devices are mostly snaps (squashfs); a disk image with a real filesystem
        // or LUKS (losetup) is shown like any other volume.
        if d.kind == "rom"
            || d.kname.starts_with("zram")
            || (d.kind == "loop" && d.fstype.as_deref() == Some("squashfs"))
        {
            continue;
        }
        let Some(fs) = d.fstype.as_deref().filter(|f| !f.is_empty()) else {
            continue;
        };
        if NOT_MOUNTABLE.contains(&fs)
            || d.parttype
                .as_deref()
                .is_some_and(|t| SYSTEM_PARTTYPES.contains(&t.to_lowercase().as_str()))
        {
            continue;
        }
        let has_children = has_child(d);
        let locked = ENCRYPTED.contains(&fs);
        if locked {
            // Unlocked = it has a mapped child; that child is listed (or mounted) on its own.
            if has_children {
                continue;
            }
        } else if has_children || d.mountpoint.as_deref().is_some_and(|m| !m.is_empty()) {
            // A disk with partitions, or something already mounted (incl. [SWAP]).
            continue;
        }
        let name = d.path.rsplit('/').next().unwrap_or(&d.path).to_string();
        out.push(Volume {
            id: d.path.clone(),
            label: d.label.clone().filter(|l| !l.is_empty()).unwrap_or(name),
            size: d.size.unwrap_or(0),
            fs: fs.to_string(),
            locked,
            removable: removable(d),
        });
    }
    out
}

#[cfg(target_os = "linux")]
static LSBLK_CACHE: std::sync::Mutex<Option<(String, Vec<BlockDev>)>> = std::sync::Mutex::new(None);

/// Clears the cached `lsblk` snapshot so the next query reads fresh device state.
pub fn invalidate_block_cache() {
    #[cfg(target_os = "linux")]
    if let Ok(mut guard) = LSBLK_CACHE.lock() {
        *guard = None;
    }
}

#[cfg(target_os = "linux")]
fn block_fingerprint() -> Option<String> {
    let parts = std::fs::read_to_string("/proc/partitions").ok()?;
    let mounts = std::fs::read_to_string("/proc/self/mounts").ok()?;
    Some(format!("{parts}\0{mounts}"))
}

#[cfg(target_os = "linux")]
fn run_lsblk() -> Vec<BlockDev> {
    let out = std::process::Command::new("lsblk")
        .args([
            "-J",
            "-b",
            "-l",
            "-o",
            "PATH,KNAME,PKNAME,TYPE,FSTYPE,LABEL,PARTTYPE,SIZE,MOUNTPOINT,RM,HOTPLUG,UUID",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => serde_json::from_slice::<Lsblk>(&o.stdout)
            .map(|l| l.blockdevices)
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Returns block devices from `lsblk`, reusing the last snapshot while `/proc/partitions`
/// and `/proc/self/mounts` are unchanged so periodic sidebar refreshes never spawn idle
/// `lsblk` processes.
#[cfg(target_os = "linux")]
fn lsblk() -> Vec<BlockDev> {
    let fp = block_fingerprint();
    let mut guard = LSBLK_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let (Some(ref key), Some((cached_key, cached))) = (&fp, guard.as_ref()) {
        if cached_key == key {
            return cached.clone();
        }
    }
    let fresh = run_lsblk();
    if let Some(key) = fp {
        *guard = Some((key, fresh.clone()));
    }
    fresh
}

/// Volumes that can be mounted or unlocked.
pub fn other_volumes() -> Vec<Volume> {
    #[cfg(target_os = "linux")]
    {
        unmounted_from(&lsblk())
    }
    #[cfg(windows)]
    {
        let mut v = win::locked_drives();
        // WSL distributions that are not running yet.
        v.extend(super::wsl::volumes());
        v
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Vec::new()
    }
}

/// Text after " at " in udisksctl output, without quotes or the final dot:
/// "Mounted /dev/sdb1 at /run/media/a/USB." → "/run/media/a/USB".
pub fn udisks_path_after(text: &str, word: &str) -> Option<String> {
    let i = text.find(word)? + word.len();
    let p = text[i..]
        .trim()
        .trim_end_matches('.')
        .trim_matches(|c| c == '`' || c == '\'' || c == '"');
    let p = p
        .lines()
        .next()
        .unwrap_or(p)
        .trim_end_matches(['.', '\'', '`']);
    (!p.is_empty()).then(|| p.to_string())
}

#[cfg(target_os = "linux")]
fn udisksctl(args: &[&str], stdin: Option<&str>) -> std::result::Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("udisksctl")
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("udisksctl: {e} (install udisks2)"))?;
    if let (Some(text), Some(mut w)) = (stdin, child.stdin.take()) {
        let _ = w.write_all(text.as_bytes()); // no newline: --key-file takes the bytes as-is
    }
    let o = child.wait_with_output().map_err(|e| e.to_string())?;
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
    if o.status.success() {
        Ok(out)
    } else {
        Err(if err.is_empty() { out } else { err })
    }
}

/// Mounts a device and returns where. Already mounted counts as success.
pub fn mount_volume(id: &str) -> Result<String> {
    // A WSL distribution ("\\wsl.localhost\Ubuntu\"): starting it mounts it.
    if cfg!(windows) {
        if let Some(w) = super::wsl::parse_unc(id) {
            super::wsl::start(&w.distro)?;
            return Ok(id.to_string());
        }
    }
    #[cfg(target_os = "linux")]
    {
        let res = match udisksctl(&["mount", "-b", id], None) {
            Ok(out) => udisks_path_after(&out, " at ")
                .ok_or_else(|| CoreError::Other(format!("mounted, but where? {out}"))),
            Err(e) if e.contains("AlreadyMounted") || e.contains("already mounted") => {
                udisks_path_after(&e, "mounted at ").ok_or(CoreError::Other(e))
            }
            Err(e) => Err(CoreError::Other(friendly(&e))),
        };
        invalidate_block_cache();
        res
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = id;
        Err(CoreError::Other("mounting is not supported here".into()))
    }
}

/// The whole disk to power off for the volume `id` (pure, for tests): the top of its parent chain.
/// Refuses while another partition of that disk (or something unlocked from it) is still mounted,
/// because the system would only answer "target is busy".
pub fn power_off_target(devs: &[BlockDev], id: &str) -> std::result::Result<String, String> {
    let start = devs
        .iter()
        .find(|d| d.path == id)
        .ok_or_else(|| format!("{id} is not connected any more"))?;
    let mut disk = start;
    while let Some(parent) = disk
        .pkname
        .as_deref()
        .and_then(|p| devs.iter().find(|x| x.kname == p))
    {
        disk = parent;
    }
    // Everything below the disk: children, and theirs.
    let mut below = vec![disk.kname.as_str()];
    let mut i = 0;
    while i < below.len() {
        let parent = below[i];
        below.extend(
            devs.iter()
                .filter(|d| d.pkname.as_deref() == Some(parent))
                .map(|d| d.kname.as_str()),
        );
        i += 1;
    }
    let busy = devs
        .iter()
        .filter(|d| below.contains(&d.kname.as_str()))
        .any(|d| d.mountpoint.as_deref().is_some_and(|m| !m.is_empty()));
    if busy {
        return Err(
            "Another volume of this drive is still mounted: unmount or eject it first".into(),
        );
    }
    Ok(disk.path.clone())
}

/// The device mounted at `mount` in an `lsblk` listing (pure, for tests).
pub fn device_in(devs: &[BlockDev], mount: &str) -> Option<String> {
    // "/run/media/a/X/" and "/run/media/a/X" are the same mount; "/" stays "/", "" matches nothing.
    fn norm(m: &str) -> Option<&str> {
        if m.is_empty() {
            return None;
        }
        Some(if m.trim_matches('/').is_empty() {
            "/"
        } else {
            m.trim_end_matches('/')
        })
    }
    let want = norm(mount)?;
    devs.iter()
        .find(|d| d.mountpoint.as_deref().and_then(norm) == Some(want))
        .map(|d| d.path.clone())
}

/// The block device mounted at `mount` (Linux), `None` for anything else (gvfs, network).
pub fn device_of_mount(mount: &str) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        device_in(&lsblk(), mount)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = mount;
        None
    }
}

/// Powers a removable drive off (`udisksctl power-off`) so it can be unplugged: the volume `id`
/// is one of its partitions, or the disk itself. It disappears from the lists afterwards.
pub fn power_off_volume(id: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        invalidate_block_cache();
        let disk = power_off_target(&lsblk(), id).map_err(CoreError::Other)?;
        let res = udisksctl(&["power-off", "-b", &disk], None)
            .map(|_| ())
            .map_err(|e| CoreError::Other(friendly(&e)));
        invalidate_block_cache();
        res
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = id;
        Err(CoreError::Other(
            "powering a drive off is not supported here".into(),
        ))
    }
}

/// First `name` on `PATH` (`which`).
#[cfg(target_os = "linux")]
fn find_program(name: &str) -> Option<std::path::PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Clears the "dirty" flag of an NTFS volume (`ntfsfix -d`, as root through polkit) and
/// mounts it. The kernel driver refuses a dirty volume ("volume is dirty and "force" flag
/// is not set"), which is what a Windows crash, a forced power-off or Fast Startup leaves
/// behind. Only NTFS volumes are touched; `ntfsfix` comes from `ntfsprogs` (Arch since 2026.9; `ntfs-3g` elsewhere).
pub fn repair_mount_volume(id: &str) -> Result<String> {
    #[cfg(target_os = "linux")]
    {
        let is_ntfs = lsblk().iter().any(|d| {
            d.path == id
                && d.fstype
                    .as_deref()
                    .is_some_and(|f| f.eq_ignore_ascii_case("ntfs"))
        });
        if !is_ntfs {
            return Err(CoreError::Other(format!("{id} is not an NTFS volume")));
        }
        let ntfsfix = find_program("ntfsfix").ok_or_else(|| {
            CoreError::Other(
                "ntfsfix not found: install the ntfsprogs package (ntfs-3g on older distributions)"
                    .into(),
            )
        })?;
        let o = std::process::Command::new("pkexec")
            .arg(ntfsfix)
            .args(["-d", id])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| CoreError::Other(format!("pkexec: {e} (install polkit)")))?;
        if !o.status.success() {
            // pkexec: 126 = dismissed or not authorized, 127 = authentication failed.
            if matches!(o.status.code(), Some(126 | 127)) {
                return Err(CoreError::Other(
                    "Not authorized to repair this volume".into(),
                ));
            }
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            return Err(CoreError::Other(format!("ntfsfix failed: {}", text.trim())));
        }
        mount_volume(id)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = id;
        Err(CoreError::Other("repairing is not supported here".into()))
    }
}

/// The mount point of `device` in an `lsblk` listing (pure, for tests).
pub fn mountpoint_in(devs: &[BlockDev], device: &str) -> Option<String> {
    devs.iter()
        .find(|d| d.path == device)
        .and_then(|d| d.mountpoint.as_deref())
        .filter(|m| !m.is_empty())
        .map(str::to_string)
}

/// Checks if the desktop automounter already mounted `clear` right after unlocking so we
/// do not race it with an extra `udisksctl mount` (and a second polkit prompt).
#[cfg(target_os = "linux")]
fn wait_for_automount(clear: &str) -> Option<String> {
    for step in 0..5 {
        invalidate_block_cache();
        if let Some(mp) = mountpoint_in(&lsblk(), clear) {
            return Some(mp);
        }
        if step < 4 {
            std::thread::sleep(std::time::Duration::from_millis(60));
        }
    }
    None
}

/// Parses KDE's `~/.config/kwalletrc` for `[Wallet] Default Wallet=...` (pure, for tests).
pub fn parse_kwallet_default(text: &str) -> Option<String> {
    let mut in_wallet = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_wallet = &line[1..line.len() - 1] == "Wallet";
            continue;
        }
        if in_wallet {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == "Default Wallet" {
                    let name = v.trim();
                    return (!name.is_empty()).then(|| name.to_string());
                }
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn kwallet_default_name() -> String {
    dirs::config_dir()
        .and_then(|c| std::fs::read_to_string(c.join("kwalletrc")).ok())
        .and_then(|text| parse_kwallet_default(&text))
        .unwrap_or_else(|| "kdewallet".into())
}

#[cfg(target_os = "linux")]
fn run_secret_lookup(program: &std::path::Path, args: &[&str]) -> (Option<i32>, Option<String>) {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return (None, None);
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return (None, None);
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        let _ = tx.send(out);
    });
    match rx.recv_timeout(std::time::Duration::from_millis(1500)) {
        Ok(out) => {
            let status = child.wait().ok();
            let code = status.and_then(|s| s.code());
            let ok = status.is_some_and(|s| s.success());
            let secret = out.trim_end_matches(['\r', '\n']);
            (code, (ok && !secret.is_empty()).then(|| secret.to_string()))
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            (None, None)
        }
    }
}

/// Desktop keyring backend used to store a saved Linux LUKS/BitLocker passphrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyringBackend {
    KWallet,
    SecretTool,
}

/// Chooses which desktop keyring tool to prefer for saving (and ordering reads) on Linux
/// (pure, for tests). Prefers KDE KWallet on KDE Plasma desktops so Dolphin/Solid share
/// the entry, and GNOME Keyring (`secret-tool`) everywhere else (Omarchy/Hyprland, GNOME,
/// XFCE, Cinnamon, COSMIC…), falling back to whichever tool is installed.
pub fn preferred_keyring(
    desktop: &str,
    has_kwallet: bool,
    has_secret_tool: bool,
) -> Option<KeyringBackend> {
    let is_kde = desktop
        .split(':')
        .any(|part| part.trim().eq_ignore_ascii_case("KDE"));
    if is_kde && has_kwallet {
        return Some(KeyringBackend::KWallet);
    }
    if has_secret_tool {
        return Some(KeyringBackend::SecretTool);
    }
    if has_kwallet {
        return Some(KeyringBackend::KWallet);
    }
    None
}

#[cfg(target_os = "linux")]
fn run_secret_store(program: &std::path::Path, args: &[&str], secret: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(secret.as_bytes()).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Looks up a LUKS/BitLocker passphrase previously saved in the user's desktop keyring:
/// - KDE Plasma KWallet (`SolidLuks` folder via `kwallet-query`, matching Dolphin/Solid)
/// - GNOME Keyring / Secret Service (`gvfs-luks-uuid` via `secret-tool`, matching Nautilus/Disks)
///
/// Checks the current desktop's preferred keyring first so non-KDE desktops do not wake
/// `kwalletd` when `secret-tool` already holds the entry.
#[cfg(target_os = "linux")]
fn lookup_saved_passphrase(id: &str) -> Option<String> {
    let dev = lsblk().into_iter().find(|d| d.path == id)?;
    let uuid = dev.uuid.as_deref().map(str::trim).filter(|u| !u.is_empty());
    let kwallet = find_program("kwallet-query");
    let secret_tool = find_program("secret-tool");
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();

    let lookup_kwallet = |tool: &std::path::Path| -> Option<String> {
        let wallet = kwallet_default_name();
        let udi = (!dev.kname.is_empty())
            .then(|| format!("/org/freedesktop/UDisks2/block_devices/{}", dev.kname));
        let keys: Vec<&str> = uuid.into_iter().chain(udi.as_deref()).collect();
        for key in keys {
            let (code, secret) = run_secret_lookup(tool, &["-f", "SolidLuks", "-r", key, &wallet]);
            if secret.is_some() {
                return secret;
            }
            // kwallet-query exit codes 1..=3 mean wallet/folder unavailable or locked;
            // only code 4 ("entry not found") warrants trying the alternate key.
            if code != Some(4) {
                break;
            }
        }
        None
    };

    let lookup_secret_tool = |tool: &std::path::Path| -> Option<String> {
        let uuid = uuid?;
        run_secret_lookup(tool, &["lookup", "gvfs-luks-uuid", uuid]).1
    };

    match preferred_keyring(&desktop, kwallet.is_some(), secret_tool.is_some()) {
        Some(KeyringBackend::KWallet) => kwallet
            .as_deref()
            .and_then(lookup_kwallet)
            .or_else(|| secret_tool.as_deref().and_then(lookup_secret_tool)),
        Some(KeyringBackend::SecretTool) => secret_tool
            .as_deref()
            .and_then(lookup_secret_tool)
            .or_else(|| kwallet.as_deref().and_then(lookup_kwallet)),
        None => None,
    }
}

/// Best-effort save of a verified LUKS/BitLocker passphrase into the desktop keyring so
/// future unlocks in Gall, Dolphin, or GNOME Files do not ask for the passphrase again.
#[cfg(target_os = "linux")]
fn store_saved_passphrase(id: &str, passphrase: &str) {
    let Some(dev) = lsblk().into_iter().find(|d| d.path == id) else {
        return;
    };
    let Some(uuid) = dev.uuid.as_deref().map(str::trim).filter(|u| !u.is_empty()) else {
        return;
    };
    let kwallet = find_program("kwallet-query");
    let secret_tool = find_program("secret-tool");
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();

    match preferred_keyring(&desktop, kwallet.is_some(), secret_tool.is_some()) {
        Some(KeyringBackend::KWallet) => {
            if let Some(tool) = kwallet {
                let wallet = kwallet_default_name();
                let _ =
                    run_secret_store(&tool, &["-f", "SolidLuks", "-w", uuid, &wallet], passphrase);
            }
        }
        Some(KeyringBackend::SecretTool) => {
            if let Some(tool) = secret_tool {
                let name = dev.label.as_deref().filter(|l| !l.is_empty()).unwrap_or(id);
                let label = format!("LUKS passphrase for {name}");
                let _ = run_secret_store(
                    &tool,
                    &["store", "--label", &label, "gvfs-luks-uuid", uuid],
                    passphrase,
                );
            }
        }
        None => {}
    }
}

/// Unlocks an encrypted device with `passphrase`, mounts it and returns where.
/// On Linux, an empty `passphrase` first checks the desktop keyring (KDE KWallet
/// `SolidLuks` and GNOME Keyring `gvfs-luks-uuid`) and returns `"Passphrase required"`
/// when no saved secret exists. When `remember` is true and a non-empty passphrase
/// succeeds, the passphrase is saved to the desktop keyring (best-effort).
/// Windows: shows the system BitLocker dialog instead (the passphrase is unused).
pub fn unlock_volume(id: &str, passphrase: &str, remember: bool) -> Result<Option<String>> {
    #[cfg(target_os = "linux")]
    {
        let owned;
        let secret = if passphrase.is_empty() {
            let Some(saved) = lookup_saved_passphrase(id) else {
                return Err(CoreError::Other("Passphrase required".into()));
            };
            owned = saved;
            owned.as_str()
        } else {
            passphrase
        };
        let mut unlocked_now = false;
        let clear = match udisksctl(
            &["unlock", "-b", id, "--key-file", "/dev/stdin"],
            Some(secret),
        ) {
            Ok(out) => {
                unlocked_now = true;
                udisks_path_after(&out, " as ")
                    .ok_or_else(|| CoreError::Other(format!("unlocked, but which device? {out}")))?
            }
            // Unlocked meanwhile (e.g. by the desktop): find the mapped device.
            Err(e) if e.contains("already unlocked") || e.contains("AlreadyUnlocked") => {
                invalidate_block_cache();
                udisks_path_after(&e, " as ")
                    .or_else(|| {
                        let devs = lsblk();
                        let kname = devs.iter().find(|d| d.path == id)?.kname.clone();
                        devs.iter()
                            .find(|d| d.pkname.as_deref() == Some(kname.as_str()))
                            .map(|d| d.path.clone())
                    })
                    .ok_or(CoreError::Other(e))?
            }
            Err(e) => return Err(CoreError::Other(friendly(&e))),
        };
        if unlocked_now && remember && !passphrase.is_empty() {
            store_saved_passphrase(id, passphrase);
        }
        if let Some(mp) = wait_for_automount(&clear) {
            return Ok(Some(mp));
        }
        mount_volume(&clear).map(Some)
    }
    #[cfg(windows)]
    {
        let _ = (passphrase, remember);
        win::unlock_dialog(id).map(|_| None)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (id, passphrase, remember);
        Err(CoreError::Other("unlocking is not supported here".into()))
    }
}

/// udisks errors are long D-Bus messages; keep the part a person can act on.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn friendly(e: &str) -> String {
    let l = e.to_lowercase();
    if l.contains("passphrase") || l.contains("no key available") || l.contains("wrong key") {
        return "Wrong passphrase".into();
    }
    if l.contains("not authorized") {
        return "Not authorized to mount this volume".into();
    }
    // "Error mounting /dev/sdb1: GDBus.Error:org.freedesktop.UDisks2.Error.Failed: Error mounting …: wrong fs type"
    e.rsplit(": ").next().unwrap_or(e).trim().to_string()
}

/// Mounts of unlocked encrypted volumes that can be locked again (not system ones
/// such as an encrypted `/` or `/home`).
pub fn lockable_mounts() -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        lockable_from(&lsblk())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

/// See [`lockable_mounts`] (pure, for tests).
pub fn lockable_from(devs: &[BlockDev]) -> Vec<String> {
    devs.iter()
        .filter(|d| {
            let parent = d
                .pkname
                .as_deref()
                .and_then(|p| devs.iter().find(|x| x.kname == p));
            parent.is_some_and(|p| p.fstype.as_deref().is_some_and(|f| ENCRYPTED.contains(&f)))
        })
        .filter_map(|d| d.mountpoint.clone())
        .filter(|m| {
            ["/run/media/", "/media/", "/mnt/"]
                .iter()
                .any(|p| m.starts_with(p))
        })
        .collect()
}

/// Unmounts an unlocked encrypted volume and locks it again.
pub fn lock_mount(mount: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        invalidate_block_cache();
        let devs = lsblk();
        let clear = devs
            .iter()
            .find(|d| d.mountpoint.as_deref() == Some(mount))
            .ok_or_else(|| CoreError::Other(format!("{mount} is not mounted")))?;
        let parent = clear
            .pkname
            .as_deref()
            .and_then(|p| devs.iter().find(|x| x.kname == p))
            .ok_or_else(|| CoreError::Other("not an encrypted volume".into()))?;
        udisksctl(&["unmount", "-b", &clear.path], None)
            .map_err(|e| CoreError::Other(friendly(&e)))?;
        let res = udisksctl(&["lock", "-b", &parent.path], None)
            .map(|_| ())
            .map_err(|e| CoreError::Other(friendly(&e)));
        invalidate_block_cache();
        res
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = mount;
        Err(CoreError::Other("locking is not supported here".into()))
    }
}

/// Windows: drive letters that exist but can't be read because BitLocker locks them.
#[cfg(windows)]
mod win {
    use super::Volume;
    use crate::error::{CoreError, Result};
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLogicalDrives() -> u32;
        fn GetDriveTypeW(root: *const u16) -> u32;
        fn GetVolumeInformationW(
            root: *const u16,
            name: *mut u16,
            name_len: u32,
            serial: *mut u32,
            max_comp: *mut u32,
            flags: *mut u32,
            fs: *mut u16,
            fs_len: u32,
        ) -> i32;
        fn GetLastError() -> u32;
    }
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: *mut std::ffi::c_void,
            op: *const u16,
            file: *const u16,
            params: *const u16,
            dir: *const u16,
            show: i32,
        ) -> isize;
    }
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const ERROR_NOT_READY: u32 = 21;

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(Some(0))
            .collect()
    }

    /// A fixed drive that can't be read is locked; a removable one only when the error
    /// is not "no media" (an empty card reader slot also can't be read).
    pub fn locked_drives() -> Vec<Volume> {
        // SAFETY: plain Win32 calls with NUL-terminated buffers that outlive them.
        let mask = unsafe { GetLogicalDrives() };
        let mut out = Vec::new();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let root = format!("{}:\\", (b'A' + i as u8) as char);
            let w = wide(&root);
            let kind = unsafe { GetDriveTypeW(w.as_ptr()) };
            if kind != DRIVE_FIXED && kind != DRIVE_REMOVABLE {
                continue;
            }
            let ok = unsafe {
                GetVolumeInformationW(
                    w.as_ptr(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                )
            };
            if ok != 0 {
                continue;
            }
            let err = unsafe { GetLastError() };
            if kind == DRIVE_REMOVABLE && err == ERROR_NOT_READY {
                continue;
            }
            out.push(Volume {
                id: root.clone(),
                label: format!("Locked drive ({})", root.trim_end_matches('\\')),
                size: 0,
                fs: "BitLocker".into(),
                locked: true,
                removable: kind == DRIVE_REMOVABLE,
            });
        }
        out
    }

    /// Shows Windows' own BitLocker unlock dialog for the drive.
    pub fn unlock_dialog(root: &str) -> Result<()> {
        crate::util::openwith::allow_foreground();
        let verb = wide("unlock-bde");
        let file = wide(root);
        // SAFETY: NUL-terminated UTF-16 strings that outlive the call.
        let r = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        };
        if r > 32 {
            return Ok(());
        }
        // No such verb (older Windows): Explorer asks for the password when the drive is opened.
        std::process::Command::new("explorer.exe")
            .arg(root)
            .spawn()
            .map(|_| ())
            .map_err(|e| CoreError::Other(format!("cannot show the BitLocker dialog: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Vec<BlockDev> {
        serde_json::from_str::<Lsblk>(json).unwrap().blockdevices
    }

    const SAMPLE: &str = r#"{"blockdevices":[
      {"path":"/dev/sda","kname":"sda","pkname":null,"type":"disk","fstype":null,"label":null,"parttype":null,"size":64000000000,"mountpoint":null,"rm":true,"hotplug":true},
      {"path":"/dev/sda1","kname":"sda1","pkname":"sda","type":"part","fstype":"ntfs","label":"Matej","parttype":"ebd0a0a2-b9e5-4433-87c0-68b6b72699c7","size":57000000000,"mountpoint":null,"rm":true,"hotplug":true},
      {"path":"/dev/sdb","kname":"sdb","pkname":null,"type":"disk","fstype":"crypto_LUKS","label":null,"parttype":null,"size":32000000000,"mountpoint":null,"rm":"1","hotplug":"1"},
      {"path":"/dev/sdc1","kname":"sdc1","pkname":"sdc","type":"part","fstype":"crypto_LUKS","label":"Backup","parttype":null,"size":1000,"mountpoint":null,"rm":false,"hotplug":true},
      {"path":"/dev/mapper/luks-1","kname":"dm-1","pkname":"sdc1","type":"crypt","fstype":"ext4","label":"Backup","parttype":null,"size":990,"mountpoint":"/run/media/a/Backup","rm":false,"hotplug":false},
      {"path":"/dev/nvme0n1p1","kname":"nvme0n1p1","pkname":"nvme0n1","type":"part","fstype":"vfat","label":null,"parttype":"c12a7328-f81f-11d2-ba4b-00a0c93ec93b","size":600,"mountpoint":null,"rm":false,"hotplug":false},
      {"path":"/dev/nvme0n1p2","kname":"nvme0n1p2","pkname":"nvme0n1","type":"part","fstype":"crypto_LUKS","label":null,"parttype":null,"size":900,"mountpoint":null,"rm":false,"hotplug":false},
      {"path":"/dev/mapper/root","kname":"dm-0","pkname":"nvme0n1p2","type":"crypt","fstype":"btrfs","label":"fedora","parttype":null,"size":890,"mountpoint":"/","rm":false,"hotplug":false},
      {"path":"/dev/nvme0n1p3","kname":"nvme0n1p3","pkname":"nvme0n1","type":"part","fstype":"swap","label":null,"parttype":null,"size":8,"mountpoint":null,"rm":false,"hotplug":false},
      {"path":"/dev/loop0","kname":"loop0","pkname":null,"type":"loop","fstype":"squashfs","label":null,"parttype":null,"size":8,"mountpoint":null,"rm":false,"hotplug":false}
    ]}"#;

    #[test]
    fn finds_unmounted_and_locked_volumes() {
        let v = unmounted_from(&parse(SAMPLE));
        let ids: Vec<_> = v
            .iter()
            .map(|x| (x.id.as_str(), x.label.as_str(), x.locked, x.removable))
            .collect();
        assert_eq!(
            ids,
            vec![
                ("/dev/sda1", "Matej", false, true),
                ("/dev/sdb", "sdb", true, true)
            ]
        );
    }

    #[test]
    fn partition_stays_listed_after_unmount() {
        // Mounted: not listed. The same partition after `unmount` (no mountpoint): listed.
        let row = |mp: &str| {
            format!(
                r#"{{"blockdevices":[{{"path":"/dev/sdd1","kname":"sdd1","pkname":"sdd","type":"part","fstype":"exfat","label":"USB","parttype":null,"size":1000,"mountpoint":{mp},"rm":true,"hotplug":true}}]}}"#
            )
        };
        assert!(unmounted_from(&parse(&row(r#""/run/media/a/USB""#))).is_empty());
        let v = unmounted_from(&parse(&row("null")));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "/dev/sdd1");
    }

    #[test]
    fn power_off_goes_to_the_whole_disk() {
        let devs = parse(SAMPLE);
        assert_eq!(
            power_off_target(&devs, "/dev/sda1").as_deref(),
            Ok("/dev/sda")
        );
        assert_eq!(
            power_off_target(&devs, "/dev/sda").as_deref(),
            Ok("/dev/sda")
        );
        // A locked partition whose mapped device is mounted keeps the disk busy.
        assert!(power_off_target(&devs, "/dev/sdc1").is_err());
        assert!(power_off_target(&devs, "/dev/gone")
            .unwrap_err()
            .contains("not connected"));
    }

    #[test]
    fn finds_the_device_of_a_mount() {
        let devs = parse(SAMPLE);
        assert_eq!(
            device_in(&devs, "/run/media/a/Backup/").as_deref(),
            Some("/dev/mapper/luks-1")
        );
        assert_eq!(device_in(&devs, "/"), Some("/dev/mapper/root".to_string()));
        assert_eq!(device_in(&devs, "/run/media/a/none"), None);
        assert_eq!(device_in(&devs, ""), None);
    }

    #[test]
    fn power_off_waits_for_other_mounted_partitions() {
        let two = r#"{"blockdevices":[
          {"path":"/dev/sde","kname":"sde","pkname":null,"type":"disk","size":1,"mountpoint":null,"rm":true,"hotplug":true},
          {"path":"/dev/sde1","kname":"sde1","pkname":"sde","type":"part","fstype":"vfat","size":1,"mountpoint":"/run/media/a/ONE","rm":true,"hotplug":true},
          {"path":"/dev/sde2","kname":"sde2","pkname":"sde","type":"part","fstype":"ext4","size":1,"mountpoint":null,"rm":true,"hotplug":true}
        ]}"#;
        let devs = parse(two);
        assert!(power_off_target(&devs, "/dev/sde2")
            .unwrap_err()
            .contains("still mounted"));
        let none = two.replace("\"/run/media/a/ONE\"", "null");
        assert_eq!(
            power_off_target(&parse(&none), "/dev/sde2").as_deref(),
            Ok("/dev/sde")
        );
    }

    #[test]
    fn lockable_only_for_unlocked_removable_style_mounts() {
        assert_eq!(
            lockable_from(&parse(SAMPLE)),
            vec!["/run/media/a/Backup".to_string()]
        );
    }

    #[test]
    fn reads_udisks_output() {
        assert_eq!(
            udisks_path_after("Mounted /dev/sdb1 at /run/media/a/My USB.\n", " at ").as_deref(),
            Some("/run/media/a/My USB")
        );
        assert_eq!(
            udisks_path_after("Unlocked /dev/sdb1 as /dev/dm-3.\n", " as ").as_deref(),
            Some("/dev/dm-3")
        );
        assert_eq!(
            udisks_path_after("Error mounting /dev/sdb1: GDBus.Error:org.freedesktop.UDisks2.Error.AlreadyMounted: Device /dev/sdb1 is already mounted at `/run/media/a/X'.", "mounted at ").as_deref(),
            Some("/run/media/a/X")
        );
        assert_eq!(friendly("Error unlocking /dev/sdb: GDBus.Error:org.freedesktop.UDisks2.Error.Failed: Error unlocking /dev/sdb: Failed to activate device: Incorrect passphrase"), "Wrong passphrase");
    }

    #[test]
    fn mountpoint_lookup_finds_mounted_clear_device() {
        let devs = parse(SAMPLE);
        assert_eq!(
            mountpoint_in(&devs, "/dev/mapper/luks-1").as_deref(),
            Some("/run/media/a/Backup")
        );
        assert_eq!(mountpoint_in(&devs, "/dev/sdc1"), None);
        assert_eq!(mountpoint_in(&devs, "/dev/no-such"), None);
    }

    #[test]
    fn parses_kwallet_default_wallet_name() {
        assert_eq!(
            parse_kwallet_default("[Wallet]\nDefault Wallet=mywallet\nEnabled=true\n").as_deref(),
            Some("mywallet")
        );
        assert_eq!(
            parse_kwallet_default(
                "[Other]\nDefault Wallet=wrong\n[Wallet]\nDefault Wallet = kdewallet\n"
            )
            .as_deref(),
            Some("kdewallet")
        );
        assert_eq!(parse_kwallet_default("[Wallet]\nDefault Wallet=\n"), None);
        assert_eq!(parse_kwallet_default("[Other]\nEnabled=true\n"), None);
    }

    #[test]
    fn picks_preferred_keyring_backend_for_desktop() {
        assert_eq!(
            preferred_keyring("KDE", true, true),
            Some(KeyringBackend::KWallet)
        );
        assert_eq!(
            preferred_keyring("fedora:KDE", true, true),
            Some(KeyringBackend::KWallet)
        );
        assert_eq!(
            preferred_keyring("KDE", false, true),
            Some(KeyringBackend::SecretTool)
        );
        assert_eq!(
            preferred_keyring("Hyprland", true, true),
            Some(KeyringBackend::SecretTool)
        );
        assert_eq!(
            preferred_keyring("GNOME", true, true),
            Some(KeyringBackend::SecretTool)
        );
        assert_eq!(
            preferred_keyring("", true, false),
            Some(KeyringBackend::KWallet)
        );
        assert_eq!(preferred_keyring("KDE", false, false), None);
    }
}

#[cfg(test)]
mod live {
    /// Prints this computer's volumes: `cargo test -p gall-core live_volumes -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_volumes() {
        println!("{:#?}", super::other_volumes());
        println!("lockable: {:?}", super::lockable_mounts());
    }
}
