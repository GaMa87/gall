//! Windows: the paths of files dropped from other apps (Explorer, the desktop).
//!
//! The webview keeps its own HTML5 drag & drop (`dragDropEnabled: false`, so drags
//! between the browsers work), but a drop from Explorer then carries only `File` objects,
//! and a web page is never told their paths. WebView2 hands such objects to the host:
//! the UI posts them with `chrome.webview.postMessageWithAdditionalObjects("gall-drop:<id>",
//! files)`, this reads each `ICoreWebView2File`'s path and answers with an
//! `external-drop` event carrying the same id. (Linux and macOS webviews give the paths
//! as a `text/uri-list` already.)

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2File, ICoreWebView2WebMessageReceivedEventArgs,
    ICoreWebView2WebMessageReceivedEventArgs2,
};
use webview2_com::{take_pwstr, WebMessageReceivedEventHandler};
use windows::core::{Interface, PWSTR};

#[derive(Clone, Serialize)]
struct Dropped {
    id: u64,
    paths: Vec<String>,
}

/// Listens for dropped files on the main window's webview.
pub fn install(app: &AppHandle) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let h = app.clone();
    let res = win.with_webview(move |wv| {
        // SAFETY: COM calls on the webview's own (main) thread, as with_webview runs them.
        unsafe {
            let Ok(core) = wv.controller().CoreWebView2() else {
                log::warn!("dropped files: no webview");
                return;
            };
            let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    on_message(&h, &args);
                }
                Ok(())
            }));
            let mut token = Default::default();
            if let Err(e) = core.add_WebMessageReceived(&handler, &mut token) {
                log::warn!("dropped files: cannot listen ({e})");
            }
        }
    });
    if let Err(e) = res {
        log::warn!("dropped files: {e}");
    }
}

/// A "gall-drop:<id>" message with files attached: their paths go back to the UI.
unsafe fn on_message(app: &AppHandle, args: &ICoreWebView2WebMessageReceivedEventArgs) {
    let mut msg = PWSTR::null();
    if args.TryGetWebMessageAsString(&mut msg).is_err() {
        return; // not a string (Tauri's own messages are strings too, but not ours)
    }
    let text = take_pwstr(msg);
    log::debug!(
        "webview message: {}",
        text.chars().take(60).collect::<String>()
    );
    let Some(id) = text
        .strip_prefix("gall-drop:")
        .and_then(|s| s.parse::<u64>().ok())
    else {
        return;
    };
    let mut paths = Vec::new();
    if let Ok(objects) = args
        .cast::<ICoreWebView2WebMessageReceivedEventArgs2>()
        .and_then(|a| a.AdditionalObjects())
    {
        let mut n = 0u32;
        let _ = objects.Count(&mut n);
        for i in 0..n {
            let Ok(obj) = objects.GetValueAtIndex(i) else {
                continue;
            };
            let Ok(file) = obj.cast::<ICoreWebView2File>() else {
                continue;
            };
            let mut p = PWSTR::null();
            if file.Path(&mut p).is_ok() {
                paths.push(take_pwstr(p));
            }
        }
    }
    log::info!("dropped files: {} path(s)", paths.len());
    let _ = app.emit("external-drop", Dropped { id, paths });
}
