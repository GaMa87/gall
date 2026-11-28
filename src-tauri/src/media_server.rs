//! Local HTTP server for video and audio previews on Linux.
//!
//! WebKitGTK cannot play `<video>`/`<audio>` from a custom URI scheme such as Tauri's
//! `asset://` (WebKit bug 146351): the player fails with "format or codec not supported"
//! for every file, whatever its codec. Plain `http://127.0.0.1` works, so media is served
//! from here instead. The server only listens on the loopback interface and every URL
//! carries a random token, so other local programs and web pages cannot read files
//! through it. Other platforms keep using the asset protocol.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Most bytes read per request head; real browsers send well under 2 KB.
const MAX_HEAD: usize = 16 * 1024;
const CHUNK: usize = 256 * 1024;
/// Maximum size of a single JPEG frame returned by `/_ff/frame/`.
const MAX_SINGLE_FRAME: usize = 8 * 1024 * 1024;
const MJPEG_BOUNDARY: &str = "gallframe";

/// Starts the server on a free loopback port and returns the URL prefix
/// (`http://127.0.0.1:PORT/TOKEN/`). The path of the file follows, percent-encoded.
pub fn start() -> std::io::Result<String> {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))?;
    let port = listener.local_addr()?.port();
    let token = random_token()?;
    let prefix = format!("/{token}/");
    let expected_host = format!("127.0.0.1:{port}");
    std::thread::Builder::new()
        .name("media-server".into())
        .spawn({
            let prefix = prefix.clone();
            move || {
                for stream in listener.incoming().flatten() {
                    let prefix = prefix.clone();
                    let host = expected_host.clone();
                    let _ =
                        std::thread::Builder::new()
                            .name("media-conn".into())
                            .spawn(move || {
                                let _ = serve(stream, &prefix, &host);
                            });
                }
            }
        })?;
    Ok(format!("http://127.0.0.1:{port}{prefix}"))
}

fn random_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 24];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[derive(Debug, PartialEq)]
struct Request {
    head_only: bool,
    target: String,
    range: Option<String>,
    host: Option<String>,
    origin: Option<String>,
}

fn parse_request(head: &str) -> Option<Request> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?;
    let target = first.next()?.to_string();
    if method != "GET" && method != "HEAD" {
        return None;
    }
    let (mut range, mut host, mut origin) = (None, None, None);
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "range" => range = Some(value.trim().to_string()),
            "host" => host = Some(value.trim().to_string()),
            "origin" => origin = Some(value.trim().to_string()),
            _ => {}
        }
    }
    Some(Request {
        head_only: method == "HEAD",
        target,
        range,
        host,
        origin,
    })
}

/// Returns CORS headers for `fetch()` requests that already passed the loopback `Host` check and
/// the per-session 192-bit secret token in the URL path. WebKitGTK sends `Origin: null` for
/// custom-scheme (`tauri://localhost`) fetches, so `*` is required after token verification.
fn cors_headers(origin: Option<&str>) -> String {
    if origin.is_some() {
        "Access-Control-Allow-Origin: *\r\n".to_string()
    } else {
        String::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct FfQuery {
    start_sec: f64,
    fps: f64,
    max_width: u32,
}

impl Default for FfQuery {
    fn default() -> Self {
        Self {
            start_sec: 0.0,
            fps: 24.0,
            max_width: 1280,
        }
    }
}

fn parse_ff_query(query: Option<&str>) -> FfQuery {
    let mut out = FfQuery::default();
    let Some(q) = query else {
        return out;
    };
    for pair in q.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        match k {
            "t" => {
                if let Ok(sec) = v.parse::<f64>() {
                    if sec.is_finite() {
                        out.start_sec = sec.clamp(0.0, 864_000.0);
                    }
                }
            }
            "fps" => {
                if let Ok(fps) = v.parse::<f64>() {
                    if fps.is_finite() {
                        out.fps = fps.clamp(1.0, 30.0);
                    }
                }
            }
            "w" => {
                if let Ok(w) = v.parse::<u32>() {
                    out.max_width = w.clamp(64, 1920);
                }
            }
            _ => {}
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FfMode {
    Frame,
    Video,
    Audio,
}

fn split_ff_route(rest: &str) -> (Option<FfMode>, &str) {
    if let Some(p) = rest.strip_prefix("_ff/frame/") {
        (Some(FfMode::Frame), p)
    } else if let Some(p) = rest.strip_prefix("_ff/video/") {
        (Some(FfMode::Video), p)
    } else if let Some(p) = rest.strip_prefix("_ff/audio/") {
        (Some(FfMode::Audio), p)
    } else {
        (None, rest)
    }
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn ffmpeg_command(args: &[String]) -> Command {
    let argv: Vec<String> = std::iter::once("ffmpeg".to_string())
        .chain(args.iter().cloned())
        .collect();
    let wrapped = gall_core::util::hostspawn::wrap_argv(argv, None, &[]);
    let mut cmd = Command::new(&wrapped[0]);
    cmd.args(&wrapped[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn serve_ffmpeg(
    stream: &mut TcpStream,
    mode: FfMode,
    path: &Path,
    q: FfQuery,
    head_only: bool,
    cors: &str,
) -> std::io::Result<()> {
    let path_str = path.to_string_lossy().into_owned();
    let ss = format!("{:.3}", q.start_sec);
    let scale = format!("scale='min({},iw)':-2", q.max_width);
    match mode {
        FfMode::Frame => {
            let args = vec![
                "-v".into(),
                "error".into(),
                "-nostdin".into(),
                "-ss".into(),
                ss,
                "-i".into(),
                path_str,
                "-an".into(),
                "-sn".into(),
                "-dn".into(),
                "-frames:v".into(),
                "1".into(),
                "-vf".into(),
                scale,
                "-pix_fmt".into(),
                "yuvj420p".into(),
                "-c:v".into(),
                "mjpeg".into(),
                "-q:v".into(),
                "4".into(),
                "-f".into(),
                "image2pipe".into(),
                "pipe:1".into(),
            ];
            let Ok(child) = ffmpeg_command(&args).spawn() else {
                return reply(stream, "503 Service Unavailable", cors);
            };
            let mut guard = KillOnDrop(child);
            let Some(stdout) = guard.0.stdout.take() else {
                return reply(stream, "500 Internal Server Error", cors);
            };
            let mut jpeg = Vec::new();
            let _ = stdout
                .take(MAX_SINGLE_FRAME as u64 + 1)
                .read_to_end(&mut jpeg);
            if jpeg.is_empty() || jpeg.len() > MAX_SINGLE_FRAME {
                return reply(stream, "404 Not Found", cors);
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n{cors}\r\n",
                jpeg.len()
            );
            stream.write_all(head.as_bytes())?;
            if !head_only {
                stream.write_all(&jpeg)?;
            }
            Ok(())
        }
        FfMode::Video | FfMode::Audio => {
            let args = match mode {
                FfMode::Video => {
                    let vf = format!("fps={:.2},{scale}", q.fps);
                    vec![
                        "-v".into(),
                        "error".into(),
                        "-nostdin".into(),
                        "-ss".into(),
                        ss,
                        "-i".into(),
                        path_str,
                        "-an".into(),
                        "-sn".into(),
                        "-dn".into(),
                        "-vf".into(),
                        vf,
                        "-pix_fmt".into(),
                        "yuvj420p".into(),
                        "-c:v".into(),
                        "mjpeg".into(),
                        "-q:v".into(),
                        "5".into(),
                        "-f".into(),
                        "mpjpeg".into(),
                        "-boundary_tag".into(),
                        MJPEG_BOUNDARY.into(),
                        "pipe:1".into(),
                    ]
                }
                FfMode::Audio => vec![
                    "-v".into(),
                    "error".into(),
                    "-nostdin".into(),
                    "-ss".into(),
                    ss,
                    "-i".into(),
                    path_str,
                    "-vn".into(),
                    "-sn".into(),
                    "-dn".into(),
                    "-ac".into(),
                    "2".into(),
                    "-ar".into(),
                    "44100".into(),
                    "-f".into(),
                    "f32le".into(),
                    "-acodec".into(),
                    "pcm_f32le".into(),
                    "pipe:1".into(),
                ],
                FfMode::Frame => unreachable!(),
            };
            let Ok(child) = ffmpeg_command(&args).spawn() else {
                return reply(stream, "503 Service Unavailable", cors);
            };
            let mut guard = KillOnDrop(child);
            let Some(mut stdout) = guard.0.stdout.take() else {
                return reply(stream, "500 Internal Server Error", cors);
            };
            // Always use `application/octet-stream` + RFC 9112 chunked transfer: WebKitGTK/libsoup3
            // aborts `fetch()` ReadableStream on `multipart/x-mixed-replace` or unframed HTTP/1.1
            // bodies with `TypeError: Load failed`.
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nCache-Control: no-store\r\nConnection: close\r\n{cors}\r\n"
            );
            stream.write_all(head.as_bytes())?;
            if head_only {
                return Ok(());
            }
            let mut buf = vec![0u8; 32 * 1024];
            loop {
                let n = stdout.read(&mut buf)?;
                if n == 0 {
                    let _ = stream.write_all(b"0\r\n\r\n");
                    let _ = stream.flush();
                    break;
                }
                let chunk_head = format!("{n:x}\r\n");
                if stream.write_all(chunk_head.as_bytes()).is_err()
                    || stream.write_all(&buf[..n]).is_err()
                    || stream.write_all(b"\r\n").is_err()
                {
                    break;
                }
            }
            Ok(())
        }
    }
}

fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(out)
}

/// First byte range of a `Range: bytes=a-b` header (`a-`, `-n` included), clamped to the
/// file. `None` means unsatisfiable; the whole-file case is the caller's (no header).
fn parse_range(header: &str, len: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    let first = spec.split(',').next()?.trim();
    let (a, b) = first.split_once('-')?;
    if len == 0 {
        return None;
    }
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        if n == 0 {
            return None;
        }
        (len.saturating_sub(n), len - 1)
    } else {
        let start: u64 = a.parse().ok()?;
        let end = if b.is_empty() {
            len - 1
        } else {
            b.parse::<u64>().ok()?.min(len - 1)
        };
        (start, end)
    };
    (start <= end && start < len).then_some((start, end))
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mp4" | "m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("mov") => "video/quicktime",
        Some("ogv") => "video/ogg",
        Some("avi") => "video/x-msvideo",
        Some("mp3") => "audio/mpeg",
        Some("ogg" | "oga" | "opus") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("flac") => "audio/flac",
        Some("m4a") => "audio/mp4",
        Some("aac") => "audio/aac",
        _ => "application/octet-stream",
    }
}

fn read_head(stream: &mut TcpStream) -> std::io::Result<Option<String>> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut head = String::new();
    loop {
        let mut line = String::new();
        let n = reader
            .by_ref()
            .take((MAX_HEAD - head.len()) as u64 + 1)
            .read_line(&mut line)?;
        if n == 0 || head.len() + n > MAX_HEAD {
            return Ok(None);
        }
        let blank = line == "\r\n" || line == "\n";
        head.push_str(&line.replace("\r\n", "\n").replace('\n', "\r\n"));
        if blank {
            return Ok(Some(head));
        }
    }
}

fn reply(stream: &mut TcpStream, status: &str, extra: &str) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n{extra}\r\n"
    )
}

fn serve(mut stream: TcpStream, prefix: &str, expected_host: &str) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let Some(head) = read_head(&mut stream)? else {
        return reply(&mut stream, "400 Bad Request", "");
    };
    let Some(req) = parse_request(&head) else {
        return reply(
            &mut stream,
            "405 Method Not Allowed",
            "Allow: GET, HEAD\r\n",
        );
    };
    // A web page that rebinds its own name to 127.0.0.1 sends a different Host.
    if req.host.as_deref() != Some(expected_host) {
        return reply(&mut stream, "403 Forbidden", "");
    }
    let Some(rest) = req.target.strip_prefix(prefix) else {
        return reply(&mut stream, "403 Forbidden", "");
    };
    let cors = cors_headers(req.origin.as_deref());
    let rest_no_hash = rest.split('#').next().unwrap_or("");
    let (route_path, query) = match rest_no_hash.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest_no_hash, None),
    };
    let (ff_mode, encoded_path) = split_ff_route(route_path);
    let Some(path) = percent_decode(encoded_path).and_then(|b| String::from_utf8(b).ok()) else {
        return reply(&mut stream, "400 Bad Request", &cors);
    };
    // The path is absolute; the URL may or may not repeat the slash after the token.
    let path = format!("/{}", path.trim_start_matches('/'));
    let path = Path::new(&path);
    // Checked before opening: opening a FIFO would block this thread.
    let len = match std::fs::metadata(path) {
        Ok(m) if m.is_file() => m.len(),
        _ => return reply(&mut stream, "404 Not Found", &cors),
    };
    if let Some(mode) = ff_mode {
        let q = parse_ff_query(query);
        return serve_ffmpeg(&mut stream, mode, path, q, req.head_only, &cors);
    }
    let Ok(mut file) = File::open(path) else {
        return reply(&mut stream, "404 Not Found", &cors);
    };
    let (status, start, end) = match &req.range {
        None => ("200 OK", 0, len.saturating_sub(1)),
        Some(r) => match parse_range(r, len) {
            Some((s, e)) => ("206 Partial Content", s, e),
            None => {
                return reply(
                    &mut stream,
                    "416 Range Not Satisfiable",
                    &format!("Content-Range: bytes */{len}\r\n{cors}"),
                )
            }
        },
    };
    let count = if len == 0 { 0 } else { end - start + 1 };
    let mut out = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {}\r\nContent-Length: {count}\r\nAccept-Ranges: bytes\r\nCache-Control: no-store\r\nConnection: close\r\n{cors}",
        content_type(path)
    );
    if status.starts_with("206") {
        out.push_str(&format!("Content-Range: bytes {start}-{end}/{len}\r\n"));
    }
    out.push_str("\r\n");
    stream.write_all(out.as_bytes())?;
    if req.head_only || count == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::Start(start))?;
    let mut left = count;
    let mut buf = vec![0u8; CHUNK];
    while left > 0 {
        let n = file.read(&mut buf[..left.min(CHUNK as u64) as usize])?;
        if n == 0 {
            break;
        }
        // The player closes the connection when it seeks; that is not an error.
        stream.write_all(&buf[..n])?;
        left -= n as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-1445", 10_000), Some((0, 1445)));
        assert_eq!(parse_range("bytes=9000-", 10_000), Some((9000, 9999)));
        assert_eq!(parse_range("bytes=-500", 10_000), Some((9500, 9999)));
        assert_eq!(parse_range("bytes=0-99999", 10_000), Some((0, 9999)));
        assert_eq!(parse_range("bytes=0-1,5-9", 10_000), Some((0, 1)));
        assert_eq!(parse_range("bytes=10000-", 10_000), None);
        assert_eq!(parse_range("bytes=5-2", 10_000), None);
        assert_eq!(parse_range("bytes=0-", 0), None);
        assert_eq!(parse_range("items=0-1", 10), None);
    }

    #[test]
    fn decodes_percent_escapes() {
        assert_eq!(
            percent_decode("/a%20b/%C4%8D.mp4").unwrap(),
            "/a b/č.mp4".as_bytes()
        );
        assert!(percent_decode("/bad%2").is_none());
        assert!(percent_decode("/bad%zz").is_none());
    }

    #[test]
    fn parses_request_head() {
        let r =
            parse_request("GET /t/x.mp4 HTTP/1.1\r\nHost: 127.0.0.1:1\r\nRange: bytes=0-1\r\n\r\n")
                .unwrap();
        assert_eq!(r.target, "/t/x.mp4");
        assert_eq!(r.range.as_deref(), Some("bytes=0-1"));
        assert_eq!(r.host.as_deref(), Some("127.0.0.1:1"));
        assert!(!r.head_only);
        assert!(parse_request("POST / HTTP/1.1\r\n\r\n").is_none());
    }

    #[test]
    fn parses_ff_routes_queries_and_cors_origins() {
        assert_eq!(
            split_ff_route("_ff/video/home/u/a.mp4"),
            (Some(FfMode::Video), "home/u/a.mp4")
        );
        assert_eq!(
            split_ff_route("_ff/frame/home/u/a.mp4"),
            (Some(FfMode::Frame), "home/u/a.mp4")
        );
        assert_eq!(
            split_ff_route("_ff/audio/home/u/a.flac"),
            (Some(FfMode::Audio), "home/u/a.flac")
        );
        assert_eq!(split_ff_route("home/u/a.mp4"), (None, "home/u/a.mp4"));

        assert_eq!(
            parse_ff_query(Some("t=3.5&fps=24&w=960")),
            FfQuery {
                start_sec: 3.5,
                fps: 24.0,
                max_width: 960,
            }
        );
        assert_eq!(
            parse_ff_query(Some("t=-5&fps=999&w=10")),
            FfQuery {
                start_sec: 0.0,
                fps: 30.0,
                max_width: 64,
            }
        );
        assert_eq!(parse_ff_query(None), FfQuery::default());

        assert_eq!(
            cors_headers(Some("http://tauri.localhost")),
            "Access-Control-Allow-Origin: *\r\n"
        );
        assert_eq!(
            cors_headers(Some("null")),
            "Access-Control-Allow-Origin: *\r\n"
        );
        assert!(cors_headers(None).is_empty());
    }

    #[test]
    fn streams_ffmpeg_frame_video_and_audio_when_ffmpeg_is_installed() {
        if Command::new("ffmpeg")
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            return;
        }
        let dir = std::env::temp_dir().join(format!("gall-media-ff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mp4 = dir.join("clip.mp4");
        let generated = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=64x64:d=0.4:r=10",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.4",
                "-c:v",
                "mpeg4",
                "-c:a",
                "aac",
                "-shortest",
                "-y",
            ])
            .arg(&mp4)
            .stdin(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        assert!(generated, "ffmpeg failed to create test mp4");

        let base = start().unwrap();
        let url = base.strip_prefix("http://").unwrap();
        let (host, prefix) = url.split_once('/').unwrap();
        let enc = mp4.to_string_lossy().trim_start_matches('/').to_string();

        let fetch = |sub: &str, origin: &str| -> (String, Vec<u8>) {
            let mut s = TcpStream::connect(host).unwrap();
            write!(
                s,
                "GET /{prefix}{sub} HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\n\r\n"
            )
            .unwrap();
            let mut raw = Vec::new();
            s.read_to_end(&mut raw).unwrap();
            let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
            (
                String::from_utf8_lossy(&raw[..split]).into_owned(),
                raw[split..].to_vec(),
            )
        };

        let (head, body) = fetch(&format!("_ff/frame/{enc}?t=0.0&w=64"), "null");
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(head.contains("Content-Type: image/jpeg"), "{head}");
        assert!(head.contains("Access-Control-Allow-Origin: *"), "{head}");
        assert!(
            body.len() > 100 && body[0] == 0xFF && body[1] == 0xD8,
            "expected JPEG frame"
        );

        let decode_chunked = |raw: &[u8]| -> Vec<u8> {
            let mut out = Vec::new();
            let mut i = 0;
            while i < raw.len() {
                let end = raw[i..].windows(2).position(|w| w == b"\r\n").unwrap() + i;
                let size_str = std::str::from_utf8(&raw[i..end]).unwrap().trim();
                let size = usize::from_str_radix(size_str, 16).unwrap();
                i = end + 2;
                if size == 0 {
                    break;
                }
                out.extend_from_slice(&raw[i..i + size]);
                i += size + 2;
            }
            out
        };

        let (head, raw_body) = fetch(
            &format!("_ff/video/{enc}?t=0.0&fps=10&w=64"),
            "tauri://localhost",
        );
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(
            head.contains("Content-Type: application/octet-stream"),
            "{head}"
        );
        assert!(head.contains("Transfer-Encoding: chunked"), "{head}");
        let body = decode_chunked(&raw_body);
        assert!(body.windows(11).any(|w| w == b"--gallframe"));
        assert!(body.windows(2).any(|w| w == [0xFF, 0xD8]));

        let (head, raw_body) = fetch(&format!("_ff/audio/{enc}?t=0.0"), "null");
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(
            head.contains("Content-Type: application/octet-stream"),
            "{head}"
        );
        assert!(head.contains("Transfer-Encoding: chunked"), "{head}");
        let body = decode_chunked(&raw_body);
        assert!(body.len() >= 1024 && body.len() % 8 == 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    fn get(base: &str, path_and_query: &str, range: Option<&str>) -> (String, Vec<u8>) {
        let url = base.strip_prefix("http://").unwrap();
        let (host, prefix) = url.split_once('/').unwrap();
        let mut s = TcpStream::connect(host).unwrap();
        let range = range.map(|r| format!("Range: {r}\r\n")).unwrap_or_default();
        write!(
            s,
            "GET /{prefix}{path_and_query} HTTP/1.1\r\nHost: {host}\r\n{range}\r\n"
        )
        .unwrap();
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).unwrap();
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        (
            String::from_utf8_lossy(&raw[..split]).into_owned(),
            raw[split..].to_vec(),
        )
    }

    #[test]
    fn serves_files_with_ranges_and_refuses_others() {
        let dir = std::env::temp_dir().join(format!("gall-media-server-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("clip één.mp4");
        std::fs::write(&file, (0..=255u8).cycle().take(5000).collect::<Vec<_>>()).unwrap();
        let base = start().unwrap();
        let enc = file
            .to_string_lossy()
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b == b'/' || b == b'.' {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect::<String>();
        let enc = enc.trim_start_matches('/');

        let (head, body) = get(&base, enc, None);
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert!(head.contains("Content-Type: video/mp4"));
        assert_eq!(body.len(), 5000);

        let (head, body) = get(&base, enc, Some("bytes=0-1445"));
        assert!(head.starts_with("HTTP/1.1 206"), "{head}");
        assert!(head.contains("Content-Range: bytes 0-1445/5000"));
        assert_eq!(body.len(), 1446);

        let (head, body) = get(&base, enc, Some("bytes=4990-"));
        assert!(head.contains("Content-Range: bytes 4990-4999/5000"));
        assert_eq!(
            body,
            (4990..5000).map(|i| (i % 256) as u8).collect::<Vec<_>>()
        );

        let (head, _) = get(&base, enc, Some("bytes=9000-"));
        assert!(head.starts_with("HTTP/1.1 416"));

        // Wrong token and a directory are refused.
        let wrong = base.replace(base.rsplit('/').nth(1).unwrap(), "nope");
        assert!(get(&wrong, enc, None).0.starts_with("HTTP/1.1 403"));
        let dir_enc = dir.to_string_lossy().trim_start_matches('/').to_string();
        assert!(get(&base, &dir_enc, None).0.starts_with("HTTP/1.1 404"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
