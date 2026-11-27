//! Wait for WebKit's resized content to reach the mapped Wayland surface.
use crate::commands::{finish_staged_window, is_staged_window};
use gtk::{glib::object::ObjectType, prelude::*};
use std::sync::atomic::{AtomicBool, Ordering};

static PRESENTATION_AVAILABLE: AtomicBool = AtomicBool::new(false);

pub(super) fn discover_on_main_thread() {
    extern "C" {
        fn install_feedback(display: *mut std::ffi::c_void) -> i32;
    }
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        let available = gdk::Display::default().is_some_and(|display| {
            // The C bridge checks the GDK backend before accessing Wayland.
            unsafe { install_feedback(display.as_ptr().cast()) != 0 }
        });
        PRESENTATION_AVAILABLE.store(available, Ordering::Release);
    }
}

pub(super) fn available() -> bool {
    PRESENTATION_AVAILABLE.load(Ordering::Acquire)
}

/// Runs on GTK's thread. Record hardware-timed presentations without forcing a repaint.
pub(super) fn frame_trace(window: &tauri::WebviewWindow, start: bool) -> Option<Vec<[u64; 2]>> {
    use std::ffi::c_void;
    extern "C" {
        fn start_frame_trace(window: *mut c_void) -> i32;
        fn stop_frame_trace(window: *mut c_void, frames: *mut [u64; 2], capacity: usize) -> usize;
    }

    let native = window.gtk_window().ok()?.window()?;
    let ptr = native.as_ptr().cast();
    if start {
        // The C bridge rejects native windows without Wayland presentation support.
        return (unsafe { start_frame_trace(ptr) } != 0).then(Vec::new);
    }

    let mut frames = vec![[0, 0]; 256];
    // The bridge copies at most the supplied capacity on this GTK thread.
    let count = unsafe { stop_frame_trace(ptr, frames.as_mut_ptr(), frames.len()) };
    frames.truncate(count);
    Some(frames)
}

fn view_geometry(view: &gtk::Widget) -> (i32, i32, i32) {
    (
        view.allocated_width(),
        view.allocated_height(),
        view.scale_factor(),
    )
}

fn pixel_difference(a: &[u8], b: &[u8]) -> u32 {
    let alpha_a = u32::from(a.get(3).copied().unwrap_or(255));
    let alpha_b = u32::from(b.get(3).copied().unwrap_or(255));
    let mut difference = alpha_a.abs_diff(alpha_b);
    for channel in 0..3 {
        difference +=
            (u32::from(a[channel]) * alpha_a / 255).abs_diff(u32::from(b[channel]) * alpha_b / 255);
    }
    difference
}

pub(super) fn prepare_first_frame(window: tauri::WebviewWindow) {
    if !is_staged_window(&window) {
        return;
    }
    let result_window = window.clone();
    if crate::report_capture::snapshot_webview(&window, move |result| {
        if !is_staged_window(&result_window) {
            return;
        }
        match result {
            Ok(surface) => wait_for_staged_buffer(result_window, surface),
            Err(error) => {
                log::info!("startup: WebKit could not sample the first frame: {error}");
                finish_staged_window(&result_window);
            }
        }
    })
    .is_err()
    {
        finish_staged_window(&window);
    }
}

fn wait_for_staged_buffer(window: tauri::WebviewWindow, surface: gdk::cairo::Surface) {
    const SAMPLE_WIDTH: i32 = 96;
    const SAMPLE_HEIGHT: i32 = 60;
    let Ok(image) = gdk::cairo::ImageSurface::try_from(surface) else {
        finish_staged_window(&window);
        return;
    };
    let Some(view) = window
        .default_vbox()
        .ok()
        .and_then(|content| content.children().into_iter().next())
    else {
        finish_staged_window(&window);
        return;
    };
    let geometry = view_geometry(&view);
    let (width, height, scale) = geometry;
    if width < 160 || height < 100 {
        finish_staged_window(&window);
        return;
    }

    if image.width() != width * scale || image.height() != height * scale {
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
            prepare_first_frame(window)
        });
        return;
    }
    let rects = [(1, 1), (2, 2), (3, 3)].map(|(col, row)| {
        (
            width * col / 4 - SAMPLE_WIDTH / 2,
            height * row / 4 - SAMPLE_HEIGHT / 2,
        )
    });
    let wanted: Option<Vec<_>> = rects
        .iter()
        .map(|&(x, y)| {
            gdk::pixbuf_get_from_surface(
                &image,
                x * scale,
                y * scale,
                SAMPLE_WIDTH * scale,
                SAMPLE_HEIGHT * scale,
            )
            .and_then(|p| p.scale_simple(24, 15, gdk::gdk_pixbuf::InterpType::Bilinear))
        })
        .collect();
    let Some(wanted) = wanted else {
        finish_staged_window(&window);
        return;
    };
    let started = std::time::Instant::now();
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(20), move || {
        if !is_staged_window(&window) {
            return gtk::glib::ControlFlow::Break;
        }
        if view_geometry(&view) != geometry {
            prepare_first_frame(window.clone());
            return gtk::glib::ControlFlow::Break;
        }
        let errors: Option<Vec<_>> = rects
            .iter()
            .zip(&wanted)
            .map(|(&(x, y), target)| {
                let rendered = gdk::cairo::ImageSurface::create(
                    gdk::cairo::Format::ARgb32,
                    SAMPLE_WIDTH * scale,
                    SAMPLE_HEIGHT * scale,
                )
                .ok()?;
                let context = gdk::cairo::Context::new(&rendered).ok()?;
                context.scale(f64::from(scale), f64::from(scale));
                context.translate(-f64::from(x), -f64::from(y));
                view.draw(&context);
                let current = gdk::pixbuf_get_from_surface(
                    &rendered,
                    0,
                    0,
                    rendered.width(),
                    rendered.height(),
                )?
                .scale_simple(24, 15, gdk::gdk_pixbuf::InterpType::Bilinear)?;
                let data = current.read_pixel_bytes();
                let expected = target.read_pixel_bytes();
                let actual = data.as_ref();
                let expected = expected.as_ref();
                let mut sum = 0_u32;
                let a_stride = current.rowstride() as usize;
                let b_stride = target.rowstride() as usize;
                let a_channels = current.n_channels() as usize;
                let b_channels = target.n_channels() as usize;
                for row in 0..15 {
                    for col in 0..24 {
                        let a = row * a_stride + col * a_channels;
                        let b = row * b_stride + col * b_channels;
                        sum += pixel_difference(
                            &actual[a..a + a_channels],
                            &expected[b..b + b_channels],
                        );
                    }
                }
                Some(sum as f64 / (24.0 * 15.0 * 4.0))
            })
            .collect();
        if errors.is_some_and(|e| {
            e.iter().filter(|&&error| error < 8.0).count() >= 2
                && e.iter().all(|&error| error < 22.0)
        }) {
            finish_after_staged_paint(window.clone(), &view, geometry);
            return gtk::glib::ControlFlow::Break;
        }
        if started.elapsed() >= std::time::Duration::from_millis(500) {
            // A layout update may make the snapshot stale while GTK is painting.
            prepare_first_frame(window.clone());
            return gtk::glib::ControlFlow::Break;
        }
        gtk::glib::ControlFlow::Continue
    });
}

fn finish_after_staged_paint(
    window: tauri::WebviewWindow,
    view: &gtk::Widget,
    geometry: (i32, i32, i32),
) {
    use std::ffi::c_void;
    extern "C" {
        fn request_feedback(
            window: *mut c_void,
            callback: unsafe extern "C" fn(u64, u32, u32, *mut c_void),
            user_data: *mut c_void,
        ) -> i32;
    }

    struct Request {
        window: tauri::WebviewWindow,
        view: gtk::Widget,
        attempts: u8,
        geometry: (i32, i32, i32),
    }

    unsafe extern "C" fn presented(timestamp: u64, _refresh: u32, _flags: u32, data: *mut c_void) {
        let request = *unsafe { Box::from_raw(data.cast::<Request>()) };
        let presented = u8::from(timestamp != 0);
        log::info!(
            "startup: Wayland presented={presented} after attempt {} at {} ms",
            request.attempts,
            crate::startup_ms()
        );
        if !is_staged_window(&request.window) {
            return;
        }
        if view_geometry(&request.view) != request.geometry {
            gtk::glib::idle_add_local_once(move || prepare_first_frame(request.window));
        } else if presented != 0 || request.attempts >= 4 {
            finish_staged_window(&request.window);
        } else {
            gtk::glib::idle_add_local_once(move || submit(request));
        }
    }

    fn submit(mut request: Request) {
        if !is_staged_window(&request.window) {
            return;
        }
        if view_geometry(&request.view) != request.geometry {
            prepare_first_frame(request.window);
            return;
        }
        let Some(gdk_window) = request.view.toplevel().and_then(|w| w.window()) else {
            finish_staged_window(&request.window);
            return;
        };
        let view = request.view.clone();
        request.attempts += 1;
        let data = Box::into_raw(Box::new(request));
        if unsafe { request_feedback(gdk_window.as_ptr().cast(), presented, data.cast()) } == 0 {
            let request = unsafe { Box::from_raw(data) };
            finish_staged_window(&request.window);
            return;
        }
        view.queue_draw();
    }

    submit(Request {
        window,
        view: view.clone(),
        attempts: 0,
        geometry,
    });
}

#[cfg(test)]
mod tests {
    use super::pixel_difference;

    #[test]
    fn samples_check_alpha_and_ignore_rgb_behind_zero_alpha() {
        assert!(pixel_difference(&[64, 64, 64], &[64, 64, 64, 0]) > 0);
        assert_eq!(pixel_difference(&[64, 64, 64], &[64, 64, 64, 255]), 0);
        assert_eq!(pixel_difference(&[1, 2, 3, 0], &[4, 5, 6, 0]), 0);
    }
}
