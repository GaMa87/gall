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
fn lsblk() -> Vec<BlockDev> {
    let out = std::process::Command::new("lsblk")
        .args([
            "-J",
            "-b",
            "-l",
            "-o",
            "PATH,KNAME,PKNAME,TYPE,FSTYPE,LABEL,PARTTYPE,SIZE,MOUNTPOINT,RM,HOTPLUG",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => serde_json::from_slice::<Lsblk>(&o.stdout)
            .map(|l| l.blockdevices)
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Volumes that can be mounted or unlocked.
pub fn other_volumes() -> Vec<Volume> {
    #[cfg(target_os = "linux")]
    {
        unmounted_from(&lsblk())
    }
    #[cfg(windows)]
    {
        win::locked_drives()
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
    #[cfg(target_os = "linux")]
    {
        match udisksctl(&["mount", "-b", id], None) {
            Ok(out) => udisks_path_after(&out, " at ")
                .ok_or_else(|| CoreError::Other(format!("mounted, but where? {out}"))),
            Err(e) if e.contains("AlreadyMounted") || e.contains("already mounted") => {
                udisks_path_after(&e, "mounted at ").ok_or(CoreError::Other(e))
            }
            Err(e) => Err(CoreError::Other(friendly(&e))),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = id;
        Err(CoreError::Other("mounting is not supported here".into()))
    }
}

/// Unlocks an encrypted device with `passphrase`, mounts it and returns where.
/// Windows: shows the system BitLocker dialog instead (the passphrase is unused).
pub fn unlock_volume(id: &str, passphrase: &str) -> Result<Option<String>> {
    #[cfg(target_os = "linux")]
    {
        let clear = match udisksctl(
            &["unlock", "-b", id, "--key-file", "/dev/stdin"],
            Some(passphrase),
        ) {
            Ok(out) => udisks_path_after(&out, " as ")
                .ok_or_else(|| CoreError::Other(format!("unlocked, but which device? {out}")))?,
            // Unlocked meanwhile (e.g. by the desktop): find the mapped device.
            Err(e) if e.contains("already unlocked") || e.contains("AlreadyUnlocked") => {
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
        // The desktop may mount it by itself right after unlocking; mount_volume handles both.
        mount_volume(&clear).map(Some)
    }
    #[cfg(windows)]
    {
        let _ = passphrase;
        win::unlock_dialog(id).map(|_| None)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (id, passphrase);
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
        udisksctl(&["lock", "-b", &parent.path], None)
            .map_err(|e| CoreError::Other(friendly(&e)))?;
        Ok(())
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
