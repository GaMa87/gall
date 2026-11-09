//! Resolve filesystem shortcuts without launching Explorer or running a program.

use crate::error::Result;
use crate::fs::FileEntry;
use std::path::Path;

/// Plain file/folder shortcuts follow Gall's normal activation. Launchers with
/// arguments and shell namespace shortcuts stay with the Windows shell.
pub fn resolve(path: &Path) -> Result<Option<FileEntry>> {
    #[cfg(windows)]
    {
        windows_shortcut::resolve(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(None)
    }
}

#[cfg(windows)]
mod windows_shortcut {
    use super::*;
    use crate::error::CoreError;
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, STGM_READ,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    struct Apartment(bool);
    impl Apartment {
        fn new() -> Result<Self> {
            let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            // Tauri's blocking workers may already be initialized as MTA. ShellLink
            // works in that apartment too, but we must not uninitialize its owner.
            if result == windows::core::HRESULT(0x80010106u32 as i32) {
                return Ok(Self(false));
            }
            result.ok().map_err(error)?;
            Ok(Self(true))
        }
    }
    impl Drop for Apartment {
        fn drop(&mut self) {
            if self.0 {
                unsafe { CoUninitialize() };
            }
        }
    }

    fn error(e: windows::core::Error) -> CoreError {
        CoreError::Other(format!("Cannot resolve Windows shortcut: {e}"))
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    fn is_shortcut(path: &Path) -> bool {
        path.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
    }

    pub(super) fn resolve(path: &Path) -> Result<Option<FileEntry>> {
        if !is_shortcut(path) {
            return Ok(None);
        }
        let _apartment = Apartment::new()?;
        let mut path = path.to_path_buf();
        for _ in 0..16 {
            let link: IShellLinkW =
                unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
                    .map_err(error)?;
            let persist: IPersistFile = link.cast().map_err(error)?;
            unsafe { persist.Load(PCWSTR(wide(&path).as_ptr()), STGM_READ) }.map_err(error)?;
            let mut args = vec![0u16; 32768];
            unsafe { link.GetArguments(&mut args) }.map_err(error)?;
            if args[0] != 0 {
                return Ok(None);
            }
            let mut target = vec![0u16; 32768];
            unsafe { link.GetPath(&mut target, std::ptr::null_mut(), 0) }.map_err(error)?;
            let len = target.iter().position(|&c| c == 0).unwrap_or(target.len());
            if len == 0 {
                return Ok(None);
            }
            path = PathBuf::from(OsString::from_wide(&target[..len]));
            if is_shortcut(&path) && !path.is_dir() {
                continue;
            }
            let path = crate::nav::path::strip_verbatim(path);
            let entry = FileEntry::from_path(&path)
                .map_err(|e| CoreError::io("resolve shortcut target", &path, e))?;
            // Preserve a launcher's working directory, elevation and other shell settings.
            if !entry.is_dir
                && path.extension().is_some_and(|ext| {
                    [
                        "exe", "com", "bat", "cmd", "ps1", "msi", "scr", "pif", "cpl",
                    ]
                    .iter()
                    .any(|candidate| ext.eq_ignore_ascii_case(candidate))
                })
            {
                return Ok(None);
            }
            return Ok(Some(entry));
        }
        Err(CoreError::Other(
            "Windows shortcut chain is cyclic or too long".into(),
        ))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Temp folders may be spelled with 8.3 short names (`RUNNER~1`); Windows resolves
        /// the long form.
        fn same_folder(path: &Path) -> std::path::PathBuf {
            crate::nav::path::canonicalize(path).unwrap()
        }

        fn save(path: &Path, target: &Path, args: Option<&str>) {
            let link: IShellLinkW =
                unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.unwrap();
            unsafe { link.SetPath(PCWSTR(wide(target).as_ptr())) }.unwrap();
            if let Some(args) = args {
                let args: Vec<u16> = args.encode_utf16().chain(Some(0)).collect();
                unsafe { link.SetArguments(PCWSTR(args.as_ptr())) }.unwrap();
            }
            let persist: IPersistFile = link.cast().unwrap();
            unsafe { persist.Save(PCWSTR(wide(path).as_ptr()), true) }.unwrap();
        }

        #[test]
        fn resolves_files_folders_and_chains_without_launching() {
            let _apartment = Apartment::new().unwrap();
            let dir = tempfile::tempdir().unwrap();
            let folder = dir.path().join("Folder with spaces");
            std::fs::create_dir(&folder).unwrap();
            let program_folder = dir.path().join("Folder.exe");
            std::fs::create_dir(&program_folder).unwrap();
            let shortcut_folder = dir.path().join("Folder.lnk");
            std::fs::create_dir(&shortcut_folder).unwrap();
            let file = folder.join("document.2do");
            std::fs::write(&file, "preview me").unwrap();
            for target in [&folder, &program_folder, &shortcut_folder, &file] {
                let link = dir.path().join("target.LNK");
                save(&link, target, None);
                let resolved = resolve(&link).unwrap().unwrap();
                assert_eq!(same_folder(Path::new(&resolved.path)), same_folder(target));
                assert_eq!(resolved.is_dir, target.is_dir());
                let chain = dir.path().join("chain.lnk");
                save(&chain, &link, None);
                assert_eq!(resolve(&chain).unwrap().unwrap().path, resolved.path);
            }
            assert!(resolve(&file).unwrap().is_none());
        }

        #[test]
        fn resolves_in_an_existing_multithreaded_apartment() {
            use windows::Win32::System::Com::COINIT_MULTITHREADED;
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
                .ok()
                .unwrap();
            let _apartment = Apartment(true);
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("document.txt");
            std::fs::write(&target, "text").unwrap();
            let link = dir.path().join("target.lnk");
            save(&link, &target, None);
            assert_eq!(
                same_folder(Path::new(&resolve(&link).unwrap().unwrap().path)),
                same_folder(&target)
            );
        }

        #[test]
        fn preserves_launchers_and_reports_broken_or_invalid_links() {
            let _apartment = Apartment::new().unwrap();
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("document.txt");
            std::fs::write(&target, "text").unwrap();
            let link = dir.path().join("target.lnk");
            save(&link, &target, Some("--argument"));
            assert!(resolve(&link).unwrap().is_none());
            let program = dir.path().join("program.exe");
            std::fs::write(&program, "not executed").unwrap();
            save(&link, &program, None);
            assert!(resolve(&link).unwrap().is_none());
            save(&link, &dir.path().join("missing.txt"), None);
            assert!(resolve(&link).is_err());
            std::fs::write(&link, "not a shortcut").unwrap();
            assert!(resolve(&link).is_err());
        }
    }
}
