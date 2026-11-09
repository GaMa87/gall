//! Capture only Gall's own webview and export explicitly selected local media.

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[cfg(any(windows, target_os = "linux", test))]
const MAX_MEDIA_BYTES: usize = gall_core::report_bundle::MAX_MEDIA_BYTES;
const MAX_CAPTURE_PIXELS: u64 = 16_777_216;
const CAPTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
type CaptureResult = Result<Vec<u8>, String>;
static CAPTURING: AtomicBool = AtomicBool::new(false);

struct CapturePermit;
impl Drop for CapturePermit {
    fn drop(&mut self) {
        CAPTURING.store(false, Ordering::Release);
    }
}

fn check_dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || u64::from(width) * u64::from(height) > MAX_CAPTURE_PIXELS
    {
        return Err(
            "Gall's window exceeds the capture dimension limit. Resize it and retry.".into(),
        );
    }
    Ok(())
}

/// This never selects a desktop, monitor, or foreground window.
#[tauri::command]
pub async fn bug_report_capture(window: tauri::WebviewWindow) -> CaptureResult {
    if CAPTURING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("A Gall capture is already in progress. Wait for it to finish.".into());
    }
    // Keep at most one native request alive, including after a caller times out.
    let permit = Arc::new(CapturePermit);
    let size = window
        .inner_size()
        .map_err(|e| format!("Cannot measure Gall's window: {e}"))?;
    check_dimensions(size.width, size.height)?;
    let (sender, receiver) = std::sync::mpsc::channel();
    start_capture(&window, sender, permit.clone())?;
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        let png = receiver
            .recv_timeout(CAPTURE_TIMEOUT)
            .map_err(|_| "Gall capture timed out. Retry, or use system capture and Add file.")??;
        super::bug_report::decode_screenshot(&png)?;
        Ok(png)
    })
    .await
    .map_err(|e| format!("Cannot capture Gall: {e}"))?
}

#[cfg(windows)]
fn start_capture(
    window: &tauri::WebviewWindow,
    sender: std::sync::mpsc::Sender<CaptureResult>,
    permit: Arc<CapturePermit>,
) -> Result<(), String> {
    use webview2_com::CapturePreviewCompletedHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
    use windows::Win32::System::Com::{
        StructuredStorage::CreateStreamOnHGlobal, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET,
    };

    window
        .with_webview(move |webview| {
            // SAFETY: all COM objects are created, used, and released on the webview's
            // own UI thread. The completion sends only owned PNG bytes to the worker.
            let result = unsafe {
                (|| -> Result<(), String> {
                    let stream = CreateStreamOnHGlobal(Default::default(), true)
                        .map_err(|e| format!("Cannot create capture stream: {e}"))?;
                    let output = stream.clone();
                    let completed = sender.clone();
                    let handler = CapturePreviewCompletedHandler::create(Box::new(move |status| {
                        let _permit = permit;
                        let result = (|| -> CaptureResult {
                            status.map_err(|e| format!("Gall webview capture failed: {e}"))?;
                            let mut stat = STATSTG::default();
                            output
                                .Stat(&mut stat, STATFLAG_NONAME)
                                .map_err(|e| format!("Cannot measure capture: {e}"))?;
                            if stat.cbSize == 0 || stat.cbSize > MAX_MEDIA_BYTES as u64 {
                                return Err("Capture exceeds the 8 MiB limit.".into());
                            }
                            output
                                .Seek(0, STREAM_SEEK_SET, None)
                                .map_err(|e| format!("Cannot read capture: {e}"))?;
                            let mut bytes = vec![0; stat.cbSize as usize];
                            let mut read = 0;
                            output
                                .Read(
                                    bytes.as_mut_ptr().cast(),
                                    bytes.len() as u32,
                                    Some(&mut read),
                                )
                                .ok()
                                .map_err(|e| format!("Cannot read capture: {e}"))?;
                            if read as usize != bytes.len() {
                                return Err("Gall capture returned incomplete image data.".into());
                            }
                            Ok(bytes)
                        })();
                        let _ = completed.send(result);
                        Ok(())
                    }));
                    webview
                        .controller()
                        .CoreWebView2()
                        .and_then(|core| {
                            core.CapturePreview(
                                COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                                &stream,
                                &handler,
                            )
                        })
                        .map_err(|e| format!("Cannot start Gall webview capture: {e}"))
                })()
            };
            if let Err(error) = result {
                let _ = sender.send(Err(error));
            }
        })
        .map_err(|e| format!("Cannot access Gall's webview: {e}"))
}

#[cfg(target_os = "linux")]
fn start_capture(
    window: &tauri::WebviewWindow,
    sender: std::sync::mpsc::Sender<CaptureResult>,
    permit: Arc<CapturePermit>,
) -> Result<(), String> {
    use gdk::glib::object::ObjectType;
    use gdk::{cairo, gio, glib};
    use std::ffi::c_void;

    // WebKitGTK is already linked by Tauri. Use its snapshot ABI through the
    // existing GTK dependencies rather than adding another WebKit crate.
    extern "C" {
        fn webkit_web_view_get_snapshot(
            webview: *mut c_void,
            region: i32,
            options: u32,
            cancellable: *mut gio::ffi::GCancellable,
            callback: Option<
                unsafe extern "C" fn(
                    *mut glib::gobject_ffi::GObject,
                    *mut gio::ffi::GAsyncResult,
                    *mut c_void,
                ),
            >,
            data: *mut c_void,
        );
        fn webkit_web_view_get_snapshot_finish(
            webview: *mut glib::gobject_ffi::GObject,
            result: *mut gio::ffi::GAsyncResult,
            error: *mut *mut glib::ffi::GError,
        ) -> *mut cairo::ffi::cairo_surface_t;
    }

    unsafe extern "C" fn completed(
        webview: *mut glib::gobject_ffi::GObject,
        result: *mut gio::ffi::GAsyncResult,
        data: *mut c_void,
    ) {
        // SAFETY: WebKit invokes this once on its GTK thread, returning the boxed
        // sender and ownership of either the native surface or the GError.
        let (sender, _permit) = *unsafe {
            Box::from_raw(
                data.cast::<(std::sync::mpsc::Sender<CaptureResult>, Arc<CapturePermit>)>(),
            )
        };
        let result = (|| -> CaptureResult {
            let mut error = std::ptr::null_mut();
            let surface =
                unsafe { webkit_web_view_get_snapshot_finish(webview, result, &mut error) };
            if !error.is_null() {
                let error: glib::Error = unsafe { glib::translate::from_glib_full(error) };
                return Err(format!("Gall webview capture failed: {error}"));
            }
            if surface.is_null() {
                return Err("Gall webview capture returned no image.".into());
            }
            let surface = unsafe { cairo::Surface::from_raw_full(surface) }
                .map_err(|e| format!("Cannot read Gall capture: {e}"))?;
            let surface = cairo::ImageSurface::try_from(surface)
                .map_err(|_| "Gall capture did not return an image surface.")?;
            let (width, height) = (surface.width(), surface.height());
            check_dimensions(width as u32, height as u32)?;
            let pixels = gdk::pixbuf_get_from_surface(&surface, 0, 0, width, height)
                .ok_or("Cannot read Gall capture pixels.")?;
            let png = pixels
                .save_to_bufferv("png", &[])
                .map_err(|e| format!("Cannot encode Gall capture: {e}"))?;
            if png.len() > MAX_MEDIA_BYTES {
                return Err("Capture exceeds the 8 MiB limit.".into());
            }
            Ok(png)
        })();
        let _ = sender.send(result);
    }

    window
        .with_webview(move |webview| {
            let sender = Box::into_raw(Box::new((sender, permit)));
            // SAFETY: with_webview runs on GTK's owning thread. Region 0 is
            // WEBKIT_SNAPSHOT_REGION_VISIBLE; options 0 excludes selection.
            unsafe {
                webkit_web_view_get_snapshot(
                    webview.inner().as_ptr().cast(),
                    0,
                    0,
                    std::ptr::null_mut(),
                    Some(completed),
                    sender.cast(),
                );
            }
        })
        .map_err(|e| format!("Cannot access Gall's webview: {e}"))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn start_capture(
    _window: &tauri::WebviewWindow,
    _sender: std::sync::mpsc::Sender<CaptureResult>,
    _permit: Arc<CapturePermit>,
) -> Result<(), String> {
    Err(
        "Gall-only capture is unavailable on this platform. Use system capture and Add file."
            .into(),
    )
}

fn save_media(path: &Path, bytes: &[u8], mime: &str) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Choose an absolute path for the media file.".into());
    }
    gall_core::report_bundle::validate_report_media(bytes, mime)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                "A file already exists at this location. Choose another name.".into()
            } else {
                format!("Cannot create media file: {error}")
            }
        })?;
    file.write_all(bytes)
        .and_then(|()| file.flush())
        .map_err(|error| format!("Cannot write media file: {error}"))
}

#[tauri::command]
pub async fn bug_report_save_media(
    path: String,
    bytes: Vec<u8>,
    mime: String,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || save_media(Path::new(&path), &bytes, &mime))
        .await
        .map_err(|e| format!("Cannot save media: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(std::path::PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::current_dir().unwrap().join(format!(
                ".report-capture-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn png() -> Vec<u8> {
        let mut output = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(2, 1))
            .write_to(&mut output, image::ImageFormat::Png)
            .unwrap();
        output.into_inner()
    }
    #[test]
    fn export_accepts_png_and_preserves_existing_file() {
        let dir = TestDir::new();
        let path = dir.0.join("capture.png");
        let bytes = png();
        save_media(&path, &bytes, "image/png").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(save_media(&path, &png(), "image/png")
            .unwrap_err()
            .contains("Choose another name"));
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn export_rejects_relative_paths_oversize_and_bad_formats_before_creation() {
        assert!(save_media(Path::new("capture.png"), &png(), "image/png")
            .unwrap_err()
            .contains("absolute"));
        #[cfg(windows)]
        for path in [r"C:capture.png", r"\capture.png"] {
            assert!(save_media(Path::new(path), &png(), "image/png")
                .unwrap_err()
                .contains("absolute"));
        }
        let dir = TestDir::new();
        let path = dir.0.join("capture.png");
        assert!(
            save_media(&path, &vec![0; MAX_MEDIA_BYTES + 1], "image/png")
                .unwrap_err()
                .contains("8 MiB")
        );
        for (bytes, mime) in [
            (b"not an image".as_slice(), "image/png"),
            (b"\x1a\x45\xdf\xa3".as_slice(), "video/webm"),
            (b"\0\0\0\x10ftypjpeg\0\0\0\0".as_slice(), "video/mp4"),
            (b"payload".as_slice(), "application/octet-stream"),
        ] {
            assert!(save_media(&path, bytes, mime).is_err());
            assert!(!path.exists());
        }
        let mut huge = png();
        huge[16..20].copy_from_slice(&8193u32.to_be_bytes());
        assert!(save_media(&path, &huge, "image/png").is_err());
        assert!(!path.exists());
    }
    #[test]
    fn recording_magic_matches_container_and_mime() {
        let dir = TestDir::new();
        let webm = b"\x1a\x45\xdf\xa3\x87\x42\x82\x84webm";
        let mp4 = b"\0\0\0\x18ftypisom\0\0\0\0isommp42";
        save_media(&dir.0.join("recording.webm"), webm, "video/webm").unwrap();
        save_media(&dir.0.join("recording.mp4"), mp4, "video/mp4").unwrap();
        assert!(save_media(&dir.0.join("mismatch.mp4"), webm, "video/mp4").is_err());
        assert!(save_media(&dir.0.join("mismatch.webm"), mp4, "video/webm").is_err());
    }
}
