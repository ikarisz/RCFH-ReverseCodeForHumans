//! Project graph (scan) and the explanation store (`.atlas/notes/**.json`).

use crate::parse::{self, Symbol};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::{fs, thread};

pub const SKIP_DIRS: &[&str] = &[
    ".git", "node_modules", "target", "dist", "build", "out", "vendor", ".venv", "venv", "env",
    "__pycache__", ".next", ".nuxt", ".svelte-kit", ".atlas", ".rsfa", ".idea", ".vscode", "coverage",
    ".turbo", ".cache", "bower_components", "Pods", ".gradle", "bin", "obj", ".mypy_cache",
    ".pytest_cache", ".tox", "site-packages", ".output", ".dart_tool", "rsfa-agent",
];

#[derive(Serialize, Deserialize, Clone)]
pub struct FileInfo {
    pub path: String,
    pub lang: String,
    pub lines: usize,
    pub hash: String,
    pub symbols: usize,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Graph {
    pub root: String,
    pub files: Vec<FileInfo>,
    pub symbols: Vec<Symbol>,
    pub unresolved_calls: usize,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Note {
    pub summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub at: u64,
}

#[derive(Serialize, Deserialize, Default)]
pub struct FileNotes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<Note>,
    #[serde(default)]
    pub symbols: BTreeMap<String, Note>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    pub lang: String,
    pub level: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { lang: "th".into(), level: "beginner".into() }
    }
}

pub fn atlas_dir(root: &Path) -> PathBuf {
    root.join(".atlas")
}

pub fn config(root: &Path) -> Config {
    fs::read_to_string(atlas_dir(root).join("config.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_config(root: &Path, c: &Config) -> Result<(), String> {
    fs::create_dir_all(atlas_dir(root)).map_err(|e| e.to_string())?;
    fs::write(atlas_dir(root).join("config.json"), serde_json::to_string_pretty(c).unwrap()).map_err(|e| e.to_string())
}

pub fn walk(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                    stack.push(e.path());
                }
            } else if ft.is_file() {
                files.push(e.path());
            }
        }
    }
    files.sort();
    files
}

pub fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

pub fn scan(root: &Path) -> Graph {
    let files: Vec<(String, &'static str, PathBuf)> = walk(root)
        .into_iter()
        .filter_map(|p| {
            let r = rel(root, &p);
            let lang = parse::lang_of(&r)?;
            let len = fs::metadata(&p).ok()?.len();
            (len < 1_000_000).then_some((r, lang, p))
        })
        .collect();
    let n = thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    let chunk = files.len().div_ceil(n).max(1);
    let parts: Vec<(FileInfo, Vec<Symbol>)> = thread::scope(|s| {
        let hs: Vec<_> = files
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .filter_map(|(r, lang, p)| {
                            let src = fs::read_to_string(p).ok()?;
                            // skip minified / generated blobs
                            if src.lines().take(5).any(|l| l.len() > 2000) {
                                return None;
                            }
                            let syms = parse::extract(r, lang, &src);
                            let hash = parse::fingerprint(lang, &src).map(|f| f.0).unwrap_or_default();
                            Some((FileInfo { path: r.clone(), lang: lang.to_string(), lines: src.lines().count(), hash, symbols: syms.len() }, syms))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    let mut g = Graph { root: root.to_string_lossy().into(), ..Default::default() };
    for (f, s) in parts {
        g.files.push(f);
        g.symbols.extend(s);
    }
    g.files.sort_by(|a, b| a.path.cmp(&b.path));
    resolve(&mut g);
    g
}

fn last_seg(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

fn family(file: &str) -> &'static str {
    match parse::lang_of(file) {
        Some("javascript" | "typescript" | "tsx") => "js",
        Some(l) => l,
        None => "",
    }
}

fn resolve(g: &mut Graph) {
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, s) in g.symbols.iter().enumerate() {
        if s.kind != "interface" && s.kind != "type" {
            by_name.entry(last_seg(&s.name).to_string()).or_default().push(i);
        }
    }
    let mut unresolved = 0;
    let mut all: Vec<Vec<String>> = Vec::with_capacity(g.symbols.len());
    for s in &g.symbols {
        let mut out = Vec::new();
        for c in &s.raw_calls {
            let member = c.starts_with('.');
            let Some(cands) = by_name.get(c.trim_start_matches('.')) else { continue };
            let fam = family(&s.file);
            let cands: Vec<usize> = cands
                .iter()
                .copied()
                .filter(|&i| family(&g.symbols[i].file) == fam && (!member || g.symbols[i].kind == "method"))
                .collect();
            let same: Vec<&usize> = cands.iter().filter(|&&i| g.symbols[i].file == s.file && g.symbols[i].id != s.id).collect();
            let pick: Vec<&usize> = if !same.is_empty() {
                same
            } else if cands.len() == 1 && g.symbols[cands[0]].id != s.id {
                vec![&cands[0]]
            } else {
                unresolved += 1;
                vec![]
            };
            for &i in pick {
                let id = g.symbols[i].id.clone();
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        all.push(out);
    }
    for (s, c) in g.symbols.iter_mut().zip(all) {
        s.calls = c;
    }
    g.unresolved_calls = unresolved;
}

pub fn save_graph(root: &Path, g: &Graph) -> Result<(), String> {
    let d = atlas_dir(root);
    fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let gi = d.join(".gitignore");
    if !gi.exists() {
        let _ = fs::write(&gi, "graph.json\nvault/\n");
    }
    fs::write(d.join("graph.json"), serde_json::to_string(g).unwrap()).map_err(|e| e.to_string())
}

pub fn load_graph(root: &Path) -> Option<Graph> {
    serde_json::from_str(&fs::read_to_string(atlas_dir(root).join("graph.json")).ok()?).ok()
}

fn notes_path(root: &Path, file: &str) -> PathBuf {
    atlas_dir(root).join("notes").join(format!("{file}.json"))
}

pub fn load_notes(root: &Path, file: &str) -> FileNotes {
    fs::read_to_string(notes_path(root, file)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_notes(root: &Path, file: &str, n: &FileNotes) -> Result<(), String> {
    let p = notes_path(root, file);
    fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&p, serde_json::to_string_pretty(n).unwrap()).map_err(|e| e.to_string())
}

pub fn load_dirs(root: &Path) -> BTreeMap<String, Note> {
    fs::read_to_string(atlas_dir(root).join("notes").join("_dirs.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_dirs(root: &Path, d: &BTreeMap<String, Note>) -> Result<(), String> {
    let p = atlas_dir(root).join("notes").join("_dirs.json");
    fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&p, serde_json::to_string_pretty(d).unwrap()).map_err(|e| e.to_string())
}

pub fn status_of(note: Option<&Note>, hash: &str) -> &'static str {
    match note {
        None => "missing",
        Some(n) if n.hash == hash => "ok",
        Some(_) => "stale",
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Does `path` fall under one of the scopes (file path, dir prefix, or symbol id)?
pub fn in_scope(path: &str, id: Option<&str>, scopes: &[String]) -> bool {
    if scopes.is_empty() {
        return true;
    }
    scopes.iter().any(|s| {
        let s = s.trim_start_matches("./").trim_end_matches('/');
        s.is_empty() || s == "." || path == s || path.starts_with(&format!("{s}/")) || id.is_some_and(|i| i == s || i.starts_with(&format!("{s}.")))
    })
}
