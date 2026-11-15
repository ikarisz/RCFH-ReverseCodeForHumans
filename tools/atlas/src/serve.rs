//! Local map app: a tiny HTTP server on 127.0.0.1 serving the embedded viewer
//! and the project's graph + explanations. Nothing leaves the machine.

use crate::store;
use serde_json::{json, Map, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use std::{fs, thread};

const INDEX: &str = include_str!("../web/index.html");

fn data(root: &Path) -> Value {
    let g = crate::fresh_graph(root).unwrap_or_default();
    let mut notes = Map::new();
    let mut files = Vec::new();
    for f in &g.files {
        let n = store::load_notes(root, &f.path);
        let fst = store::status_of(n.file.as_ref(), &f.hash);
        files.push(json!({"path": f.path, "lang": f.lang, "lines": f.lines, "status": fst,
            "summary": n.file.as_ref().map(|x| x.summary.clone()), "detail": n.file.as_ref().map(|x| x.detail.clone())}));
        for s in g.symbols.iter().filter(|s| s.file == f.path) {
            let note = n.symbols.get(&s.id);
            notes.insert(s.id.clone(), json!({"status": store::status_of(note, &s.hash), "summary": note.map(|x| &x.summary), "detail": note.map(|x| &x.detail)}));
        }
    }
    let name = root.canonicalize().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string())).unwrap_or_default();
    json!({"name": name, "config": store::config(root), "files": files, "symbols": g.symbols, "notes": notes, "dirs": store::load_dirs(root)})
}

fn version(root: &Path) -> u64 {
    let mut v = 0u64;
    let mut stack: Vec<PathBuf> = vec![store::atlas_dir(root).join("notes")];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                v = v.wrapping_add(m.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0));
            }
        }
    }
    v
}

fn query(q: &str, k: &str) -> Option<String> {
    q.split('&').find_map(|kv| {
        let (a, b) = kv.split_once('=')?;
        (a == k).then(|| decode(b))
    })
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = (b[i + 1] as char).to_digit(16).zip((b[i + 2] as char).to_digit(16));
            if let Some((x, y)) = h {
                out.push((x * 16 + y) as u8);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn respond(mut s: TcpStream, status: &str, ctype: &str, body: &[u8]) {
    let h = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len());
    let _ = s.write_all(h.as_bytes());
    let _ = s.write_all(body);
}

fn handle(root: &Path, s: TcpStream) {
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut first = String::new();
    if r.read_line(&mut first).is_err() {
        return;
    }
    // drain headers
    let mut host_ok = true;
    loop {
        let mut l = String::new();
        if r.read_line(&mut l).unwrap_or(0) == 0 || l == "\r\n" || l == "\n" {
            break;
        }
        let ll = l.to_ascii_lowercase();
        if let Some(h) = ll.strip_prefix("host:") {
            let h = h.trim();
            host_ok = h.starts_with("127.0.0.1") || h.starts_with("localhost");
        }
    }
    if !host_ok {
        return respond(s, "403 Forbidden", "text/plain", b"forbidden");
    }
    let target = first.split_whitespace().nth(1).unwrap_or("/");
    let (path, q) = target.split_once('?').unwrap_or((target, ""));
    match path {
        "/" | "/index.html" => respond(s, "200 OK", "text/html; charset=utf-8", INDEX.as_bytes()),
        "/api/data" => respond(s, "200 OK", "application/json", serde_json::to_string(&data(root)).unwrap().as_bytes()),
        "/api/version" => respond(s, "200 OK", "application/json", version(root).to_string().as_bytes()),
        "/api/source" => {
            let Some(file) = query(q, "path") else { return respond(s, "400 Bad Request", "text/plain", b"path") };
            // only files that are part of the scanned project
            let known = store::load_graph(root).is_some_and(|g| g.files.iter().any(|f| f.path == file));
            if !known {
                return respond(s, "404 Not Found", "text/plain", b"unknown file");
            }
            let text = fs::read_to_string(root.join(&file)).unwrap_or_default();
            let from: usize = query(q, "from").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
            let to: usize = query(q, "to").and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
            let body: Vec<&str> = text.lines().skip(from - 1).take(to.saturating_sub(from) + 1).collect();
            respond(s, "200 OK", "text/plain; charset=utf-8", body.join("\n").as_bytes())
        }
        _ => respond(s, "404 Not Found", "text/plain", b"not found"),
    }
}

pub fn run(root: &Path, port: u16, open_browser: bool) -> Result<(), String> {
    crate::fresh_graph(root)?;
    let l = TcpListener::bind(("127.0.0.1", port)).map_err(|e| format!("port {port}: {e} (try --port)"))?;
    let url = format!("http://127.0.0.1:{port}");
    crate::out(&json!({"ok": true, "url": url, "stop": "Ctrl+C"}));
    if open_browser {
        let _ = if cfg!(windows) {
            std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn()
        } else if cfg!(target_os = "macos") {
            std::process::Command::new("open").arg(&url).spawn()
        } else {
            std::process::Command::new("xdg-open").arg(&url).spawn()
        };
    }
    let root = root.to_path_buf();
    for s in l.incoming().flatten() {
        let r = root.clone();
        thread::spawn(move || handle(&r, s));
    }
    Ok(())
}
