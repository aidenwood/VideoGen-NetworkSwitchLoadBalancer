// LTX Mac Farm — the web gateway.
//
// WHY. The menubar app can only ever help the Mac it's running on, and only the
// person sitting at it. A render farm is a team thing: the producer wants to see
// the board from their own desk, the person who set up Mac 3 wants to finish it
// from Mac 1, and everyone wants to watch a finished clip without mounting an
// SMB share. So the same app also serves its UI over HTTP.
//
// WHAT IT IS NOT. Not a second UI — it serves the EXACT same ui/index.html the
// popover uses. The page detects whether it's inside Tauri; if it isn't, every
// call goes to POST /api/invoke instead of the Tauri bridge. One file, one
// command surface, so a feature can't exist in one place and not the other.
//
// SECURITY, PLAINLY. This gateway can start scripts and write into the shared
// queue, so it is deliberately hard to reach:
//   • bound to 127.0.0.1 by default — nothing outside this Mac can connect;
//   • LAN access is opt-in per Mac (Settings → Web gateway → "share on the LAN");
//   • once on the LAN, a random 32-hex-character key is required on every
//     request. The tray's "Copy team link" puts the key in the URL for you;
//   • file downloads are confined to the farm folder and to media extensions,
//     so ?path= can't be used to read the rest of the disk.
// It is HTTP, not HTTPS, and the key travels in the URL — fine for a private
// office switch, not something to port-forward to the internet.

use std::io::{Read, Seek, SeekFrom};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use tiny_http::{Header, Request, Response, Server, StatusCode};

use crate::Core;

// The built frontend, embedded by build.rs (see ui_assets.rs in OUT_DIR).
include!(concat!(env!("OUT_DIR"), "/ui_assets.rs"));

/// The page a browser gets at `/`. Read out of the embedded table rather than
/// `include_str!`, because Vite content-hashes its filenames.
pub fn page() -> Option<&'static [u8]> {
    UI_FILES.iter().find(|(r, _, _)| *r == "/index.html").map(|(_, b, _)| *b)
}

/// Look up any embedded asset by request path.
pub fn asset(route: &str) -> Option<(&'static [u8], &'static str)> {
    UI_FILES.iter().find(|(r, _, _)| *r == route).map(|(_, b, m)| (*b, *m))
}

/// Shown when the binary was built without a frontend — a plain page that says
/// what to run, rather than a blank window and a 404 in the log.
const NO_UI: &str = "<!doctype html><meta charset=utf-8><title>LTX Mac Farm</title>\
<body style=\"font:14px -apple-system,system-ui;background:#0f1117;color:#e7e9ee;padding:40px\">\
<h1 style=\"font-size:18px\">The interface isn't built into this binary.</h1>\
<p style=\"color:#8b90a0\">Run <code style=\"color:#38bdf8\">npm run build</code> in \
<code>desktop/ui-react</code>, then rebuild the app.</p>";
// The home-screen icon for the installable (PWA) version of the board.
const ICON: &[u8] = include_bytes!("../icons/256x256.png");

// An upload has to be big enough for a phone photo and small enough that a
// mistake can't fill the share.
const MAX_UPLOAD: u64 = 48 * 1024 * 1024;

// How many requests can be in flight at once. The UI polls every 2s per open
// tab; four threads is plenty for an office and keeps a hung osascript action
// from blocking the board.
const THREADS: usize = 4;

// Only these can be downloaded through /file, and only from inside the farm
// folder: the renders, the proof stills, the metadata and the logs.
const SERVEABLE: [(&str, &str); 8] = [
    ("mp4", "video/mp4"),
    ("mov", "video/quicktime"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("webp", "image/webp"),
    ("json", "application/json"),
    ("log", "text/plain; charset=utf-8"),
];

fn header(k: &str, v: &str) -> Header {
    // Both sides are ASCII literals we control, so this cannot fail in practice.
    Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap_or_else(|_| {
        Header::from_bytes(&b"X-Ignored"[..], &b"1"[..]).unwrap()
    })
}

fn json_response(code: u16, body: String) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body)
        .with_status_code(StatusCode(code))
        .with_header(header("Content-Type", "application/json; charset=utf-8"))
        .with_header(header("Cache-Control", "no-store"))
}

/// A random key, from the OS. No `rand` crate for one 16-byte read.
pub fn new_token() -> String {
    let mut buf = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_err()
    {
        // Fall back to something unpredictable enough to not be a fixed string.
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(1);
        let pid = std::process::id() as u128;
        let mix = t ^ (pid << 64) ^ (t << 7);
        buf.copy_from_slice(&mix.to_le_bytes());
    }
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

// Length-then-bytes compare that doesn't bail on the first mismatch. The key
// only ever crosses a LAN, but a timing-safe compare costs nothing.
fn key_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn is_loopback(addr: Option<&SocketAddr>) -> bool {
    match addr {
        Some(a) => a.ip().is_loopback(),
        // tiny_http only returns None for non-IP transports; treat unknown as remote.
        None => false,
    }
}

// ---------------------------------------------------------------------------
// URL bits — a tiny query parser, since we have exactly two query params.
// ---------------------------------------------------------------------------

fn path_of(url: &str) -> &str {
    url.split('?').next().unwrap_or("/")
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

pub fn query(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    for pair in q.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return Some(url_decode(v));
        }
    }
    None
}

// `equiv` wants a 'static str — every caller passes a literal anyway.
fn cookie(req: &Request, name: &str) -> Option<String> {
    let raw = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Cookie"))?
        .value
        .as_str()
        .to_string();
    for part in raw.split(';') {
        let (k, v) = part.trim().split_once('=')?;
        if k == name {
            return Some(v.to_string());
        }
    }
    None
}

fn header_value(req: &Request, name: &'static str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

/// Why a request is (not) allowed. Split out so it can be unit-tested without
/// a socket — the auth rule is the one thing here that must never regress.
#[derive(Debug, PartialEq)]
pub enum Access {
    Local,          // this Mac's own browser — no key needed
    Keyed,          // right key, and it came in the URL: hand back a cookie
    KeyedCookie,    // right key, already stored
    Denied,
}

pub fn decide(loopback: bool, expected: &str, from_query: Option<&str>, from_header: Option<&str>, from_cookie: Option<&str>) -> Access {
    if loopback {
        return Access::Local;
    }
    if expected.is_empty() {
        return Access::Denied; // no key configured = LAN access is off
    }
    if from_query.map(|k| key_eq(k, expected)).unwrap_or(false) {
        return Access::Keyed;
    }
    if from_header.map(|k| key_eq(k, expected)).unwrap_or(false) {
        return Access::KeyedCookie;
    }
    if from_cookie.map(|k| key_eq(k, expected)).unwrap_or(false) {
        return Access::KeyedCookie;
    }
    Access::Denied
}

// ---------------------------------------------------------------------------
// /file — hand a finished render to the browser
// ---------------------------------------------------------------------------

fn mime_for(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    SERVEABLE.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)
}

/// Resolve a requested path against the farm folder and refuse anything that
/// escapes it. Symlinks included: we canonicalize BOTH sides and compare, so a
/// link inside the share pointing at ~/.ssh doesn't become a download.
pub fn safe_media_path(root: &str, want: &str) -> Result<PathBuf, String> {
    if want.trim().is_empty() {
        return Err("no path".into());
    }
    let p = PathBuf::from(want);
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err("path escapes the farm folder".into());
    }
    let rootc = std::fs::canonicalize(root).map_err(|_| "farm folder is not reachable".to_string())?;
    let full = if p.is_absolute() { p } else { rootc.join(p) };
    let fullc = std::fs::canonicalize(&full).map_err(|_| format!("{} not found", full.display()))?;
    if !fullc.starts_with(&rootc) {
        return Err("only files inside the farm folder can be opened".into());
    }
    if !fullc.is_file() {
        return Err("not a file".into());
    }
    if mime_for(&fullc).is_none() {
        return Err("that file type isn't served".into());
    }
    Ok(fullc)
}

// "bytes=100-" / "bytes=100-200" -> (start, end_inclusive)
fn parse_range(raw: &str, len: u64) -> Option<(u64, u64)> {
    let spec = raw.trim().strip_prefix("bytes=")?;
    let (s, e) = spec.split_once('-')?;
    let start: u64 = if s.is_empty() { 0 } else { s.trim().parse().ok()? };
    let end: u64 = if e.trim().is_empty() { len.saturating_sub(1) } else { e.trim().parse().ok()? };
    if start > end || start >= len {
        return None;
    }
    Some((start, end.min(len.saturating_sub(1))))
}

fn serve_file(req: Request, path: PathBuf, download: bool) {
    let mime = mime_for(&path).unwrap_or("application/octet-stream");
    let Ok(mut file) = std::fs::File::open(&path) else {
        let _ = req.respond(json_response(404, "{\"ok\":false,\"error\":\"can't open that file\"}".into()));
        return;
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();

    let mut headers = vec![
        header("Content-Type", mime),
        header("Accept-Ranges", "bytes"),
        header("Cache-Control", "no-cache"),
    ];
    if download {
        headers.push(header("Content-Disposition", &format!("attachment; filename=\"{}\"", name)));
    }

    // Scrubbing a video is a Range request. Without this, Safari refuses to
    // play at all and Chrome can only play from the start.
    let range = header_value(&req, "Range").and_then(|r| parse_range(&r, len));
    match range {
        Some((start, end)) => {
            let take = end - start + 1;
            if file.seek(SeekFrom::Start(start)).is_err() {
                let _ = req.respond(Response::empty(StatusCode(416)));
                return;
            }
            headers.push(header("Content-Range", &format!("bytes {}-{}/{}", start, end, len)));
            let body = file.take(take);
            let _ = req.respond(Response::new(StatusCode(206), headers, body, Some(take as usize), None));
        }
        None => {
            let _ = req.respond(Response::new(StatusCode(200), headers, file, Some(len as usize), None));
        }
    }
}

// ---------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------

pub struct Gateway {
    pub port: u16,
    pub lan: bool,
    pub token: String,
    server: Arc<Server>,
}

impl Gateway {
    pub fn local_url(&self) -> String {
        format!("http://127.0.0.1:{}/?k={}", self.port, self.token)
    }
    pub fn lan_url(&self, host: &str) -> String {
        format!("http://{}.local:{}/?k={}", crate::safe_host(host), self.port, self.token)
    }
    /// Release the port. `unblock` makes every waiting `recv()` return an error,
    /// so the serving threads fall out of their loop and the socket closes —
    /// that's what lets Settings change the port without quitting the app.
    pub fn stop(&self) {
        self.server.unblock();
    }
}

/// Bind the gateway and serve it on background threads. Returns what it ended
/// up with — the port may not be the one asked for, and saying so beats a tray
/// link that points at nothing.
///
/// Port hunting matters here: two Macs is fine (different machines), but a
/// second copy of the app, or anything else already on 8787, would otherwise
/// leave the gateway silently dead.
pub fn start(core: Arc<Core>, want_port: u16, lan: bool, token: String) -> Result<Gateway, String> {
    let host_part = if lan { "0.0.0.0" } else { "127.0.0.1" };
    let mut server: Option<Server> = None;
    let mut chosen = want_port;
    for offset in 0..10u16 {
        let port = want_port.saturating_add(offset);
        match Server::http(format!("{}:{}", host_part, port)) {
            Ok(s) => {
                server = Some(s);
                chosen = port;
                break;
            }
            Err(_) => continue,
        }
    }
    let server = server.ok_or_else(|| {
        format!("Couldn't bind a port between {} and {} — something else is using them.", want_port, want_port + 9)
    })?;

    let server = Arc::new(server);
    for _ in 0..THREADS {
        let s = server.clone();
        let core = core.clone();
        let token = token.clone();
        std::thread::spawn(move || loop {
            match s.recv() {
                Ok(req) => handle(req, &core, &token),
                Err(_) => break, // unblock() or a dead socket — stop serving
            }
        });
    }
    Ok(Gateway { port: chosen, lan, token, server })
}

fn handle(mut req: Request, core: &Arc<Core>, token: &str) {
    let url = req.url().to_string();
    let path = path_of(&url).to_string();

    // /healthz answers before auth so a script can check the app is alive.
    if path == "/healthz" {
        let _ = req.respond(json_response(200, "{\"ok\":true}".into()));
        return;
    }

    let access = decide(
        is_loopback(req.remote_addr()),
        token,
        query(&url, "k").as_deref(),
        header_value(&req, "X-Farm-Key").as_deref(),
        cookie(&req, "farm_key").as_deref(),
    );
    if access == Access::Denied {
        // Say what's wrong without confirming whether a key exists.
        let body = "{\"ok\":false,\"error\":\"This farm gateway needs the team link. Ask whoever set it up for the link with the ?k=… key, or open it on the Mac itself.\"}";
        let _ = req.respond(json_response(403, body.into()));
        return;
    }

    match (req.method().as_str(), path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => {
            let body = page().map(|b| String::from_utf8_lossy(b).to_string()).unwrap_or_else(|| NO_UI.to_string());
            let mut res = Response::from_string(body)
                .with_header(header("Content-Type", "text/html; charset=utf-8"))
                .with_header(header("Cache-Control", "no-store"));
            // Remember the key so a refresh (or a link the user pastes without
            // the query) keeps working for the session.
            if access == Access::Keyed {
                res = res.with_header(header(
                    "Set-Cookie",
                    &format!("farm_key={}; Path=/; Max-Age=2592000; SameSite=Lax", token),
                ));
            }
            let _ = req.respond(res);
        }
        // --- poster frames -------------------------------------------------
        // A Done card without a thumbnail is just a filename; with one, the board
        // is a review surface. Generated on demand with ffmpeg and cached on the
        // share next to the render, so every Mac's browser reuses the same jpg.
        ("GET", "/poster") => {
            let root = core.cfg.lock().map(|c| c.root()).unwrap_or_default();
            let want = query(&url, "path").unwrap_or_default();
            match poster_for(&root, &want) {
                Ok(p) => serve_file(req, p, false),
                Err(e) => {
                    let out = serde_json::json!({"ok": false, "error": e});
                    let _ = req.respond(json_response(404, out.to_string()));
                }
            }
        }
        // --- drop an image in to make an image-to-video job ------------------
        ("POST", "/upload") => {
            let root = core.cfg.lock().map(|c| c.root()).unwrap_or_default();
            let name = query(&url, "name").unwrap_or_default();
            let target = match crate::jobs::asset_target(&root, &name) {
                Ok(t) => t,
                Err(e) => {
                    let out = serde_json::json!({"ok": false, "error": e});
                    let _ = req.respond(json_response(400, out.to_string()));
                    return;
                }
            };
            let mut buf: Vec<u8> = Vec::new();
            if req.as_reader().take(MAX_UPLOAD + 1).read_to_end(&mut buf).is_err() {
                let _ = req.respond(json_response(400, "{\"ok\":false,\"error\":\"the upload didn't arrive in one piece\"}".into()));
                return;
            }
            if buf.len() as u64 > MAX_UPLOAD {
                let out = serde_json::json!({"ok": false,
                    "error": format!("that file is over the {}MB limit", MAX_UPLOAD / 1024 / 1024)});
                let _ = req.respond(json_response(413, out.to_string()));
                return;
            }
            match std::fs::write(&target, &buf) {
                Ok(_) => {
                    let fname = target.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    let out = serde_json::json!({"ok": true, "data": {
                        "name": fname,
                        "message": format!("{} is on the share — pick it as the starting image.", fname)
                    }});
                    let _ = req.respond(json_response(200, out.to_string()));
                }
                Err(e) => {
                    let out = serde_json::json!({"ok": false,
                        "error": format!("couldn't write to assets/ — {}", e)});
                    let _ = req.respond(json_response(500, out.to_string()));
                }
            }
        }
        // --- installable on a phone ----------------------------------------
        ("GET", "/manifest.json") => {
            let body = serde_json::json!({
                "name": "LTX Mac Farm",
                "short_name": "Farm",
                "description": "The render farm's job board",
                "start_url": format!("/?k={}", token),
                "scope": "/",
                "display": "standalone",
                "background_color": "#0f1117",
                "theme_color": "#0f1117",
                "icons": [{ "src": "/icon.png", "sizes": "256x256", "type": "image/png", "purpose": "any maskable" }],
            });
            let _ = req.respond(json_response(200, body.to_string()));
        }
        ("GET", "/icon.png") => {
            let res = Response::from_data(ICON)
                .with_header(header("Content-Type", "image/png"))
                .with_header(header("Cache-Control", "max-age=86400"));
            let _ = req.respond(res);
        }
        ("GET", "/api/ping") => {
            let body = serde_json::json!({
                "ok": true,
                "host": crate::this_host(),
                "version": env!("CARGO_PKG_VERSION"),
                "surface": "web",
            });
            let _ = req.respond(json_response(200, body.to_string()));
        }
        ("POST", "/api/invoke") => {
            let mut body = String::new();
            if req.as_reader().take(1_048_576).read_to_string(&mut body).is_err() {
                let _ = req.respond(json_response(400, "{\"ok\":false,\"error\":\"unreadable body\"}".into()));
                return;
            }
            let parsed: serde_json::Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(e) => {
                    let out = serde_json::json!({"ok": false, "error": format!("bad JSON: {}", e)});
                    let _ = req.respond(json_response(400, out.to_string()));
                    return;
                }
            };
            let cmd = parsed.get("cmd").and_then(|c| c.as_str()).unwrap_or("").to_string();
            let args = parsed.get("args").cloned().unwrap_or(serde_json::Value::Null);
            let (code, out) = match core.dispatch(&cmd, &args) {
                Ok(data) => (200, serde_json::json!({"ok": true, "data": data})),
                Err(e) => (200, serde_json::json!({"ok": false, "error": e})),
            };
            let _ = req.respond(json_response(code, out.to_string()));
        }
        ("GET", "/file") => {
            let root = core.cfg.lock().map(|c| c.root()).unwrap_or_default();
            let want = query(&url, "path").unwrap_or_default();
            match safe_media_path(&root, &want) {
                Ok(p) => serve_file(req, p, query(&url, "dl").as_deref() == Some("1")),
                Err(e) => {
                    let out = serde_json::json!({"ok": false, "error": e});
                    let _ = req.respond(json_response(404, out.to_string()));
                }
            }
        }
        ("OPTIONS", _) => {
            let _ = req.respond(Response::empty(StatusCode(204)));
        }
        // The bundle. Vite content-hashes these names, so they can be cached
        // hard: a new build is a new filename.
        ("GET", path) if asset(path).is_some() => {
            let (bytes, mime) = asset(path).expect("checked");
            let res = Response::from_data(bytes)
                .with_header(header("Content-Type", mime))
                .with_header(header("Cache-Control", "public, max-age=31536000, immutable"));
            let _ = req.respond(res);
        }
        _ => {
            let _ = req.respond(json_response(404, "{\"ok\":false,\"error\":\"no such endpoint\"}".into()));
        }
    }
}

/// The cached poster frame for a finished render, generated if it isn't there.
///
/// Cached in `done/.thumbs/` rather than a temp dir because every teammate's
/// browser asks for the same frame: one ffmpeg run per clip for the whole team.
fn poster_for(root: &str, want: &str) -> Result<PathBuf, String> {
    let mp4 = safe_media_path(root, want)?;
    let ext = mp4.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext != "mp4" && ext != "mov" {
        return Err("posters are only made for video".into());
    }
    let stem = mp4.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let dir = Path::new(root).join("done/.thumbs");
    std::fs::create_dir_all(&dir).map_err(|e| format!("can't write thumbnails — {}", e))?;
    let thumb = dir.join(format!("{}.jpg", crate::jobs::safe_id(&stem)));

    let fresh = thumb.is_file() && crate::mtime_age(&thumb) <= crate::mtime_age(&mp4);
    if !fresh {
        let ff = crate::which_login("ffmpeg")
            .ok_or("ffmpeg isn't installed on this Mac, so it can't make thumbnails")?;
        // One frame a second in, scaled down: small enough to send over WiFi to
        // a phone, big enough to judge a shot.
        let out = std::process::Command::new(ff)
            .args(["-nostdin", "-loglevel", "error", "-y", "-ss", "1", "-i"])
            .arg(&mp4)
            .args(["-frames:v", "1", "-vf", "scale=480:-2", "-q:v", "5"])
            .arg(&thumb)
            .output()
            .map_err(|e| format!("ffmpeg wouldn't run — {}", e))?;
        if !thumb.is_file() {
            return Err(format!(
                "couldn't make a thumbnail: {}",
                String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("unknown error")
            ));
        }
    }
    Ok(thumb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_never_needs_a_key() {
        assert_eq!(decide(true, "", None, None, None), Access::Local);
        assert_eq!(decide(true, "abc", Some("wrong"), None, None), Access::Local);
    }

    #[test]
    fn lan_requires_the_exact_key() {
        let k = "0123456789abcdef0123456789abcdef";
        assert_eq!(decide(false, k, Some(k), None, None), Access::Keyed);
        assert_eq!(decide(false, k, None, Some(k), None), Access::KeyedCookie);
        assert_eq!(decide(false, k, None, None, Some(k)), Access::KeyedCookie);
        assert_eq!(decide(false, k, Some("nope"), None, None), Access::Denied);
        assert_eq!(decide(false, k, None, None, None), Access::Denied);
        // a prefix of the real key must not pass
        assert_eq!(decide(false, k, Some(&k[..8]), None, None), Access::Denied);
        // and with no key configured, nothing off-box gets in
        assert_eq!(decide(false, "", Some(""), None, None), Access::Denied);
    }

    #[test]
    fn tokens_are_random_and_hex() {
        let a = new_token();
        let b = new_token();
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn query_and_path_split_cleanly() {
        assert_eq!(path_of("/file?path=/a/b.mp4&dl=1"), "/file");
        assert_eq!(query("/file?path=/a/b.mp4&dl=1", "path").unwrap(), "/a/b.mp4");
        assert_eq!(query("/file?path=/a/b.mp4&dl=1", "dl").unwrap(), "1");
        assert_eq!(query("/?k=abc", "k").unwrap(), "abc");
        assert_eq!(query("/", "k"), None);
        // percent-encoded spaces in a real farm path
        assert_eq!(query("/file?path=/Volumes/Render%20Farm/done/a%20b.mp4", "path").unwrap(),
                   "/Volumes/Render Farm/done/a b.mp4");
    }

    #[test]
    fn ranges_parse_the_forms_browsers_actually_send() {
        assert_eq!(parse_range("bytes=0-", 100), Some((0, 99)));
        assert_eq!(parse_range("bytes=10-19", 100), Some((10, 19)));
        assert_eq!(parse_range("bytes=90-500", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=200-", 100), None);
        assert_eq!(parse_range("chars=0-1", 100), None);
    }

    #[test]
    fn media_paths_cannot_leave_the_farm_folder() {
        let root = std::env::temp_dir().join("ltxweb_media");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("done")).unwrap();
        std::fs::write(root.join("done/clip.mp4"), b"x").unwrap();
        std::fs::write(root.join("done/notes.txt"), b"x").unwrap();
        let r = root.to_string_lossy().to_string();

        let ok = safe_media_path(&r, &root.join("done/clip.mp4").to_string_lossy()).unwrap();
        assert!(ok.ends_with("clip.mp4"));
        // relative paths resolve inside the share
        assert!(safe_media_path(&r, "done/clip.mp4").is_ok());
        // and everything else is refused
        assert!(safe_media_path(&r, "../../../etc/passwd").is_err());
        assert!(safe_media_path(&r, "/etc/passwd").is_err());
        assert!(safe_media_path(&r, &root.join("done/notes.txt").to_string_lossy()).is_err(),
            "txt is not in the serveable list");
        assert!(safe_media_path(&r, "done/missing.mp4").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    // The gateway and Tauri must serve the SAME build. When the frontend hasn't
    // been built this is empty on purpose — the served page says so — so the test
    // asserts the shape either way rather than failing a bare `cargo test`.
    #[test]
    fn the_embedded_bundle_is_self_consistent() {
        if UI_FILES.is_empty() {
            assert!(NO_UI.contains("npm run build"), "the fallback must say what to run");
            return;
        }
        let html = page().expect("a built frontend must contain index.html");
        let html = String::from_utf8_lossy(html);
        assert!(html.contains("LTX Mac Farm"));
        // every asset the page references must be in the table
        for needle in ["src=\"./", "href=\"./"] {
            for part in html.split(needle).skip(1) {
                let name = part.split('"').next().unwrap_or("");
                if name.is_empty() || name.starts_with("http") {
                    continue;
                }
                let route = format!("/{}", name.trim_start_matches("./"));
                assert!(asset(&route).is_some(), "{} is referenced but not embedded", route);
            }
        }
    }
}
