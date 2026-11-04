//! Fallback for videos the webview cannot play (WebKitGTK plays through the system's
//! GStreamer, WebView2 only through its own codecs): ProRes, DNxHR, HEVC, … from cameras
//! and editors are common. With FFmpeg installed (optional) we can still say which codec
//! it is and show a still frame, the way file managers' thumbnailers do.

use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoProbe {
    /// `std::env::consts::OS`, so the UI can name the right codec package.
    pub platform: String,
    /// ffprobe was found and ran.
    pub ffmpeg: bool,
    /// Short codec name (`prores`, `hevc`, `dnxhd`, …).
    pub codec: Option<String>,
    /// Human name (`Apple ProRes (iCodec Pro)`).
    pub codec_long: Option<String>,
    /// Profile (`HQ`, `Main 10`, …).
    pub profile: Option<String>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    /// Seconds.
    pub duration: Option<f64>,
    /// A cached JPEG still frame.
    pub poster: Option<String>,
}

/// A console tool, without the console window flash on Windows.
fn tool(name: &str) -> Command {
    let mut cmd = Command::new(name);
    cmd.stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Codec, size and length of the first video stream, plus a still frame cached in `cache`.
pub fn probe(path: &Path, cache: &Path) -> VideoProbe {
    let mut p = VideoProbe {
        platform: std::env::consts::OS.to_string(),
        ..Default::default()
    };
    let Ok(out) = tool("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .arg("stream=codec_name,codec_long_name,profile,width,height:format=duration")
        .args(["-of", "json"])
        .arg(path)
        .output()
    else {
        return p; // no FFmpeg on this system
    };
    p.ffmpeg = true;
    parse_probe(&out.stdout, &mut p);
    p.poster = poster(path, cache, p.duration).map(|f| f.to_string_lossy().into_owned());
    p
}

fn parse_probe(json: &[u8], p: &mut VideoProbe) {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(json) else {
        return;
    };
    let text = |x: &serde_json::Value| {
        x.as_str()
            .filter(|s| !s.is_empty() && *s != "unknown")
            .map(String::from)
    };
    if let Some(s) = v["streams"].get(0) {
        p.codec = text(&s["codec_name"]);
        p.codec_long = text(&s["codec_long_name"]);
        p.profile = text(&s["profile"]);
        p.width = s["width"].as_u64();
        p.height = s["height"].as_u64();
    }
    p.duration = v["format"]["duration"]
        .as_str()
        .and_then(|d| d.parse().ok());
}

/// One frame a little into the video (10 %, at most 30 s), at most 1280 px wide.
fn poster(path: &Path, cache: &Path, duration: Option<f64>) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let mut h = DefaultHasher::new();
    (path, meta.len(), meta.modified().ok()).hash(&mut h);
    let out = cache.join(format!("{:016x}.jpg", h.finish()));
    if out.is_file() {
        return Some(out);
    }
    std::fs::create_dir_all(cache).ok()?;
    let tmp = out.with_extension("part.jpg");
    let at = duration.map(|d| (d * 0.1).min(30.0)).unwrap_or(0.0);
    let ok = tool("ffmpeg")
        .args(["-v", "error", "-ss", &format!("{at:.2}"), "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale='min(1280,iw)':-2",
            "-pix_fmt",
            "yuvj420p",
            "-q:v",
            "3",
            "-y",
        ])
        .arg(&tmp)
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok || std::fs::rename(&tmp, &out).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffprobe_json() {
        let json = br#"{"programs":[],"streams":[{"codec_name":"prores","codec_long_name":"Apple ProRes (iCodec Pro)","profile":"HQ","width":3840,"height":2160}],"format":{"duration":"125.480000"}}"#;
        let mut p = VideoProbe::default();
        parse_probe(json, &mut p);
        assert_eq!(p.codec.as_deref(), Some("prores"));
        assert_eq!(p.profile.as_deref(), Some("HQ"));
        assert_eq!((p.width, p.height), (Some(3840), Some(2160)));
        assert_eq!(p.duration, Some(125.48));
    }

    #[test]
    fn tolerates_garbage_and_missing_fields() {
        let mut p = VideoProbe::default();
        parse_probe(b"not json", &mut p);
        assert!(p.codec.is_none());
        parse_probe(
            br#"{"streams":[{"codec_name":"hevc","profile":"unknown"}],"format":{}}"#,
            &mut p,
        );
        assert_eq!(p.codec.as_deref(), Some("hevc"));
        assert!(p.profile.is_none() && p.duration.is_none());
    }
}
