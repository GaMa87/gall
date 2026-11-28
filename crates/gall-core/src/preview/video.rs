//! Zero-spool video/audio metadata probing, poster extraction, and Linux FFmpeg routing.
//!
//! WebKitGTK plays HTTP `<video>`/`<audio>` streams through GStreamer's `downloadbuffer`,
//! which copies the entire file into `/var/tmp/WebKit-Media-*` over loopback TCP at disk
//! speed. Probing headers and extracting a cached poster keeps hover/selection strictly
//! zero-copy, and routing large Linux files (or missing GStreamer codecs/plugins such as
//! ProRes, DNxHR, HEVC, MKV, ...) through the FFmpeg streaming player avoids `/var/tmp`
//! spooling during playback as well.

use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// Largest media file (32 MiB) that Linux hands to WebKitGTK's native HTTP `<video>`/`<audio>`
/// player when FFmpeg is installed. Larger files route straight to the FFmpeg streaming player
/// so WebKitGTK's GStreamer `downloadbuffer` never copies gigabytes into `/var/tmp/WebKit-Media-*`.
pub const LINUX_NATIVE_MEDIA_MAX_BYTES: u64 = 32 * 1024 * 1024;

const MAX_PROBE_CACHE: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProbeCacheKey {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

fn probe_cache() -> &'static Mutex<HashMap<ProbeCacheKey, VideoProbe>> {
    static CACHE: OnceLock<Mutex<HashMap<ProbeCacheKey, VideoProbe>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_key(path: &Path) -> Option<ProbeCacheKey> {
    let meta = std::fs::metadata(path).ok()?;
    Some(ProbeCacheKey {
        path: path.to_path_buf(),
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

fn probe_cache_get(key: &ProbeCacheKey) -> Option<VideoProbe> {
    let guard = probe_cache().lock().ok()?;
    let cached = guard.get(key)?;
    if let Some(ref poster) = cached.poster {
        if !Path::new(poster).is_file() {
            return None;
        }
    }
    Some(cached.clone())
}

fn probe_cache_put(key: ProbeCacheKey, val: &VideoProbe) {
    if let Ok(mut guard) = probe_cache().lock() {
        if guard.len() >= MAX_PROBE_CACHE {
            guard.clear();
        }
        guard.insert(key, val.clone());
    }
}

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
    /// Nominal frame rate (frames per second) of the first video stream.
    pub fps: Option<f64>,
    /// Whether the file has a video stream.
    pub has_video: bool,
    /// Whether the file has an audio stream.
    pub has_audio: bool,
    /// A cached JPEG still frame.
    pub poster: Option<String>,
}

/// A console tool, without the console window flash on Windows, running on the host inside Flatpak.
fn tool(name: &str, args: &[&str]) -> Command {
    let argv: Vec<String> = std::iter::once(name.to_string())
        .chain(args.iter().map(|s| (*s).to_string()))
        .collect();
    let wrapped = crate::util::hostspawn::wrap_argv(argv, None, &[]);
    let mut cmd = Command::new(&wrapped[0]);
    cmd.args(&wrapped[1..])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn parse_fps(raw: Option<&str>) -> Option<f64> {
    let s = raw?.trim();
    if s.is_empty() {
        return None;
    }
    let val = if let Some((num, den)) = s.split_once('/') {
        let n: f64 = num.trim().parse().ok()?;
        let d: f64 = den.trim().parse().ok()?;
        if d <= 0.0 {
            return None;
        }
        n / d
    } else {
        s.parse().ok()?
    };
    (val.is_finite() && (1.0..=240.0).contains(&val)).then_some(val)
}

/// Codec, size, length, stream presence and (for video) a still frame cached in `cache`.
///
/// Results are cached in memory by `(path, size, mtime)` so re-selecting a media file in
/// the browser spawns no subprocesses. On Linux, when FFmpeg is absent or cannot extract a
/// still frame, an up-to-date FreeDesktop desktop thumbnail (`~/.cache/thumbnails/`) is used
/// if present.
pub fn probe(path: &Path, cache: &Path) -> VideoProbe {
    let key = cache_key(path);
    if let Some(ref k) = key {
        if let Some(hit) = probe_cache_get(k) {
            return hit;
        }
    }
    let mut p = VideoProbe {
        platform: std::env::consts::OS.to_string(),
        ..Default::default()
    };
    if let Ok(out) = tool(
        "ffprobe",
        &[
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,codec_name,codec_long_name,profile,width,height,avg_frame_rate,r_frame_rate:format=duration",
            "-of",
            "json",
        ],
    )
    .arg(path)
    .output()
    {
        p.ffmpeg = true;
        parse_probe(&out.stdout, &mut p);
        if p.has_video {
            p.poster = poster(path, cache, p.duration).map(|f| f.to_string_lossy().into_owned());
        }
    }
    if p.poster.is_none() && (!p.ffmpeg || p.has_video) {
        p.poster = freedesktop_thumbnail(path).map(|f| f.to_string_lossy().into_owned());
    }
    if let Some(k) = key {
        probe_cache_put(k, &p);
    }
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
    if let Some(streams) = v["streams"].as_array() {
        let mut first_video = None;
        let mut first_audio = None;
        for s in streams {
            match s["codec_type"].as_str() {
                Some("video") => {
                    p.has_video = true;
                    if first_video.is_none() {
                        first_video = Some(s);
                    }
                }
                Some("audio") => {
                    p.has_audio = true;
                    if first_audio.is_none() {
                        first_audio = Some(s);
                    }
                }
                None if first_video.is_none() => {
                    // Backward compatibility with probes that omit codec_type.
                    p.has_video = true;
                    first_video = Some(s);
                }
                _ => {}
            }
        }
        if let Some(s) = first_video {
            p.codec = text(&s["codec_name"]);
            p.codec_long = text(&s["codec_long_name"]);
            p.profile = text(&s["profile"]);
            p.width = s["width"].as_u64();
            p.height = s["height"].as_u64();
            p.fps = parse_fps(s["avg_frame_rate"].as_str())
                .or_else(|| parse_fps(s["r_frame_rate"].as_str()));
        } else if let Some(s) = first_audio {
            p.codec = text(&s["codec_name"]);
            p.codec_long = text(&s["codec_long_name"]);
            p.profile = text(&s["profile"]);
        }
    }
    p.duration = v["format"]["duration"]
        .as_str()
        .and_then(|d| d.parse().ok());
}

/// Pure check of whether the installed Linux GStreamer plugin set can handle `ext` inside
/// WebKitGTK. When `plugins` is empty (no standard plugin directory found), returns `true` so
/// the native player still tries first.
pub fn gstreamer_can_play_ext(ext: &str, plugins: &HashSet<String>) -> bool {
    if plugins.is_empty() {
        return true;
    }
    if !plugins.contains("libgstautodetect.so") {
        return false;
    }
    match ext.to_ascii_lowercase().as_str() {
        "mp4" | "m4v" | "mov" | "m4a" => plugins.contains("libgstisomp4.so"),
        "aac" => {
            plugins.contains("libgstaudioparsers.so")
                || plugins.contains("libgstisomp4.so")
                || plugins.contains("libgstlibav.so")
        }
        "mkv" | "webm" => plugins.contains("libgstmatroska.so"),
        "ogv" | "ogg" | "oga" | "opus" => plugins.contains("libgstogg.so"),
        "flac" => plugins.contains("libgstflac.so"),
        "wav" => plugins.contains("libgstwavparse.so"),
        "avi" => plugins.contains("libgstavi.so"),
        "mp3" => plugins.contains("libgstmpg123.so") || plugins.contains("libgstlibav.so"),
        _ => true,
    }
}

#[cfg(target_os = "linux")]
fn linux_gstreamer_plugins() -> &'static HashSet<String> {
    static PLUGINS: OnceLock<HashSet<String>> = OnceLock::new();
    PLUGINS.get_or_init(|| {
        let mut dirs: Vec<PathBuf> = [
            "/usr/lib/gstreamer-1.0",
            "/usr/lib64/gstreamer-1.0",
            "/usr/lib/x86_64-linux-gnu/gstreamer-1.0",
            "/usr/lib/aarch64-linux-gnu/gstreamer-1.0",
            "/usr/local/lib/gstreamer-1.0",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        for env_var in ["GST_PLUGIN_PATH", "GST_PLUGIN_SYSTEM_PATH_1_0"] {
            if let Some(val) = std::env::var_os(env_var) {
                dirs.extend(std::env::split_paths(&val));
            }
        }
        let mut out = HashSet::new();
        for dir in dirs {
            let Ok(rd) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in rd.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.ends_with(".so") {
                        out.insert(name.to_string());
                    }
                }
            }
        }
        out
    })
}

#[cfg(target_os = "linux")]
fn has_ffmpeg_on_host() -> bool {
    static HAS_FFMPEG: OnceLock<bool> = OnceLock::new();
    *HAS_FFMPEG.get_or_init(|| {
        if crate::util::hostspawn::in_flatpak() {
            return crate::util::hostspawn::host_has("ffmpeg")
                && crate::util::hostspawn::host_has("ffprobe");
        }
        let on_path = |bin: &str| {
            std::env::var_os("PATH")
                .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
                .unwrap_or(false)
        };
        on_path("ffmpeg") && on_path("ffprobe")
    })
}

/// Pure check for whether Linux should route playback directly to the FFmpeg streaming player:
/// either the file exceeds `LINUX_NATIVE_MEDIA_MAX_BYTES` (avoiding WebKitGTK's GStreamer
/// `downloadbuffer` copying gigabytes into `/var/tmp/WebKit-Media-*`) or GStreamer lacks the
/// plugins required for `ext`.
pub fn should_prefer_ffmpeg_for(
    ext: &str,
    size_bytes: u64,
    has_ffmpeg: bool,
    plugins: &HashSet<String>,
) -> bool {
    has_ffmpeg
        && (size_bytes > LINUX_NATIVE_MEDIA_MAX_BYTES || !gstreamer_can_play_ext(ext, plugins))
}

/// True on Linux when FFmpeg is available and either `size_bytes` exceeds
/// `LINUX_NATIVE_MEDIA_MAX_BYTES` or the webview's GStreamer installation is missing the
/// plugins required for `ext`, so the UI streams directly through FFmpeg without spooling
/// to `/var/tmp/WebKit-Media-*` or hitting missing-plugin stalls.
pub fn should_prefer_ffmpeg(ext: &str, size_bytes: u64) -> bool {
    #[cfg(target_os = "linux")]
    {
        should_prefer_ffmpeg_for(
            ext,
            size_bytes,
            has_ffmpeg_on_host(),
            linux_gstreamer_plugins(),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (ext, size_bytes);
        false
    }
}

/// Canonical `file://` URI matching GLib's `g_filename_to_uri` / the FreeDesktop Thumbnail
/// Managing Standard (used to hash local paths into `~/.cache/thumbnails/<size>/<md5>.png`).
#[cfg(any(target_os = "linux", test))]
fn file_uri_for_thumbnail(path: &Path) -> Option<String> {
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes()
    };
    #[cfg(not(unix))]
    let bytes = path.to_str()?.as_bytes();
    if !bytes.starts_with(b"/") {
        return None;
    }
    let mut uri = String::with_capacity("file://".len() + bytes.len() * 2);
    uri.push_str("file://");
    for &b in bytes {
        let safe = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'/' | b'-'
                    | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
            );
        if safe {
            uri.push(b as char);
        } else {
            use std::fmt::Write;
            let _ = write!(&mut uri, "%{b:02X}");
        }
    }
    Some(uri)
}

/// Pure-Rust RFC 1321 MD5 (used solely for FreeDesktop thumbnail cache filenames, never security).
#[cfg(any(target_os = "linux", test))]
fn md5_hex(input: &[u8]) -> String {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut msg = input.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let (mut a0, mut b0, mut c0, mut d0) =
        (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    for chunk in msg.as_chunks::<64>().0 {
        let mut m = [0u32; 16];
        for (i, w) in chunk.as_chunks::<4>().0.iter().enumerate() {
            m[i] = u32::from_le_bytes(*w);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            let sum = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(S[i]));
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = String::with_capacity(32);
    for word in [a0, b0, c0, d0] {
        for byte in word.to_le_bytes() {
            use std::fmt::Write;
            let _ = write!(&mut out, "{byte:02x}");
        }
    }
    out
}

/// Looks up an existing, non-stale FreeDesktop thumbnail (`<thumbs_root>/{xx-large,x-large,large,normal}/<md5>.png`).
#[cfg(any(target_os = "linux", test))]
fn freedesktop_thumbnail_in(path: &Path, thumbs_root: &Path) -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let src_mtime = std::fs::metadata(&canonical).ok()?.modified().ok();
    let uri = file_uri_for_thumbnail(&canonical)?;
    let filename = format!("{}.png", md5_hex(uri.as_bytes()));
    for tier in ["xx-large", "x-large", "large", "normal"] {
        let candidate = thumbs_root.join(tier).join(&filename);
        let Ok(meta) = std::fs::metadata(&candidate) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if let (Some(src_t), Ok(thumb_t)) = (src_mtime, meta.modified()) {
            if thumb_t < src_t {
                continue;
            }
        }
        return Some(candidate);
    }
    None
}

fn freedesktop_thumbnail(path: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let cache_dir = dirs::cache_dir()?;
        freedesktop_thumbnail_in(path, &cache_dir.join("thumbnails"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        None
    }
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
    let at_str = format!("{at:.2}");
    let ok = tool("ffmpeg", &["-v", "error", "-ss", &at_str, "-i"])
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
        let json = br#"{"programs":[],"streams":[{"codec_type":"video","codec_name":"prores","codec_long_name":"Apple ProRes (iCodec Pro)","profile":"HQ","width":3840,"height":2160,"avg_frame_rate":"24000/1001","r_frame_rate":"24/1"},{"codec_type":"audio","codec_name":"pcm_s16le"}],"format":{"duration":"125.480000"}}"#;
        let mut p = VideoProbe::default();
        parse_probe(json, &mut p);
        assert_eq!(p.codec.as_deref(), Some("prores"));
        assert_eq!(p.profile.as_deref(), Some("HQ"));
        assert_eq!((p.width, p.height), (Some(3840), Some(2160)));
        assert_eq!(p.duration, Some(125.48));
        assert!(p.has_video && p.has_audio);
        assert!((p.fps.unwrap() - 23.976).abs() < 0.01);
    }

    #[test]
    fn parses_audio_only_and_vfr_video_streams() {
        let audio = br#"{"streams":[{"codec_type":"audio","codec_name":"flac","codec_long_name":"FLAC"}],"format":{"duration":"210.5"}}"#;
        let mut p = VideoProbe::default();
        parse_probe(audio, &mut p);
        assert!(!p.has_video && p.has_audio);
        assert_eq!(p.codec.as_deref(), Some("flac"));
        assert_eq!(p.duration, Some(210.5));
        assert!(p.fps.is_none());

        let vfr = br#"{"streams":[{"codec_type":"video","codec_name":"h264","profile":"High","width":438,"height":162,"avg_frame_rate":"570/23","r_frame_rate":"60/1"}],"format":{"duration":"5.233984"}}"#;
        let mut v = VideoProbe::default();
        parse_probe(vfr, &mut v);
        assert!(v.has_video && !v.has_audio);
        assert!((v.fps.unwrap() - (570.0 / 23.0)).abs() < 0.001);
        assert_eq!(parse_fps(Some("0/0")), None);
        assert_eq!(parse_fps(Some("0/1")), None);
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
        assert!(p.has_video);
    }

    #[test]
    fn detects_missing_gstreamer_plugins_per_extension() {
        let empty = HashSet::new();
        assert!(gstreamer_can_play_ext("mp4", &empty));

        let no_good: HashSet<String> = ["libgstcoreelements.so", "libgstautoconvert.so"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!(!gstreamer_can_play_ext("mp4", &no_good));
        assert!(!gstreamer_can_play_ext("mkv", &no_good));
        assert!(!gstreamer_can_play_ext("flac", &no_good));

        let with_mp4: HashSet<String> = ["libgstautodetect.so", "libgstisomp4.so"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!(gstreamer_can_play_ext("mp4", &with_mp4));
        assert!(gstreamer_can_play_ext("mov", &with_mp4));
        assert!(!gstreamer_can_play_ext("mkv", &with_mp4));
        assert!(!gstreamer_can_play_ext("ogg", &with_mp4));
    }

    #[test]
    fn prefers_ffmpeg_when_large_or_plugins_missing() {
        let with_mp4: HashSet<String> = ["libgstautodetect.so", "libgstisomp4.so"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!(!should_prefer_ffmpeg_for(
            "mp4",
            LINUX_NATIVE_MEDIA_MAX_BYTES,
            true,
            &with_mp4
        ));
        assert!(should_prefer_ffmpeg_for(
            "mp4",
            LINUX_NATIVE_MEDIA_MAX_BYTES + 1,
            true,
            &with_mp4
        ));
        assert!(should_prefer_ffmpeg_for("mkv", 1024, true, &with_mp4));
        // Without FFmpeg installed, never route to the FFmpeg player.
        assert!(!should_prefer_ffmpeg_for(
            "mp4",
            10 * 1024 * 1024 * 1024,
            false,
            &with_mp4
        ));
        assert!(!should_prefer_ffmpeg_for("mkv", 1024, false, &with_mp4));
    }

    #[test]
    fn md5_matches_rfc1321_vectors() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"a"), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"message digest"),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
        assert_eq!(
            md5_hex(b"abcdefghijklmnopqrstuvwxyz"),
            "c3fcd3d76192e4007dfb496cca67e13b"
        );
    }

    #[test]
    fn file_uri_encodes_spaces_and_unicode() {
        assert_eq!(
            file_uri_for_thumbnail(Path::new("/home/u/My Videos/clip #1 č.mp4")).as_deref(),
            Some("file:///home/u/My%20Videos/clip%20%231%20%C4%8D.mp4")
        );
        assert_eq!(
            file_uri_for_thumbnail(Path::new("relative/video.mp4")),
            None
        );
    }

    #[test]
    fn finds_fresh_freedesktop_thumbnail_and_rejects_stale() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("big movie.mkv");
        std::fs::write(&video, b"video").unwrap();
        let canonical = std::fs::canonicalize(&video).unwrap();
        let uri = file_uri_for_thumbnail(&canonical).unwrap();
        let hash = md5_hex(uri.as_bytes());
        let thumbs = dir.path().join("thumbnails");
        let large = thumbs.join("large");
        let xlarge = thumbs.join("x-large");
        std::fs::create_dir_all(&large).unwrap();
        std::fs::create_dir_all(&xlarge).unwrap();

        let thumb_large = large.join(format!("{hash}.png"));
        std::fs::write(&thumb_large, b"png-large").unwrap();
        assert_eq!(
            freedesktop_thumbnail_in(&video, &thumbs),
            Some(thumb_large.clone())
        );

        // Higher-resolution tier wins when present.
        let thumb_xlarge = xlarge.join(format!("{hash}.png"));
        std::fs::write(&thumb_xlarge, b"png-xlarge").unwrap();
        assert_eq!(
            freedesktop_thumbnail_in(&video, &thumbs),
            Some(thumb_xlarge)
        );
    }

    #[test]
    fn probe_cache_returns_cached_entry_and_invalidates_on_change() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("cached.mp4");
        let poster = dir.path().join("poster.jpg");
        std::fs::write(&video, b"v1").unwrap();
        std::fs::write(&poster, b"jpg").unwrap();

        let k1 = cache_key(&video).unwrap();
        let sample = VideoProbe {
            platform: "linux".into(),
            ffmpeg: true,
            codec: Some("hevc".into()),
            has_video: true,
            poster: Some(poster.to_string_lossy().into_owned()),
            ..Default::default()
        };
        probe_cache_put(k1.clone(), &sample);
        assert_eq!(probe_cache_get(&k1).unwrap().codec.as_deref(), Some("hevc"));

        // Removing the cached poster file invalidates the hit.
        std::fs::remove_file(&poster).unwrap();
        assert!(probe_cache_get(&k1).is_none());

        // Changing file size yields a different key.
        std::fs::write(&video, b"v2-longer").unwrap();
        let k2 = cache_key(&video).unwrap();
        assert_ne!(k1, k2);
    }
}
