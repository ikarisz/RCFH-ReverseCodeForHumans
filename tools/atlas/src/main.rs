//! atlas — CodeAtlas CLI. A local map of a codebase that AI agents annotate
//! with plain-language explanations, on demand (never automatically).
//!
//! The scanner (tree-sitter) builds the structure deterministically; the agent
//! only writes explanations for what `pending` hands it. Explanations live in
//! `.atlas/notes/` (sidecar, never touches code) and can optionally be written
//! into the source as comments, guarded by a comment-free AST equality check.

mod check;
mod inline;
mod mcp;
mod parse;
mod serve;
mod store;

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::{env, fs};
use store::{FileNotes, Graph, Note};

pub fn out(v: &Value) {
    println!("{}", serde_json::to_string(v).unwrap());
}

fn fail(msg: impl Into<String>) -> ExitCode {
    out(&json!({"ok": false, "error": msg.into()}));
    ExitCode::from(1)
}

pub fn fresh_graph(root: &Path) -> Result<Graph, String> {
    let g = store::scan(root);
    store::save_graph(root, &g)?;
    Ok(g)
}

pub fn git_changed(root: &Path) -> Vec<String> {
    let o = Command::new("git").args(["-C"]).arg(root).args(["status", "--porcelain", "-uall"]).output();
    let Ok(o) = o else { return vec![] };
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| l.get(3..))
        .map(|p| p.rsplit(" -> ").next().unwrap_or(p).trim_matches('"').replace('\\', "/"))
        .collect()
}

fn top_dirs(path: &str) -> Vec<String> {
    let parts: Vec<&str> = path.split('/').collect();
    (1..parts.len()).take(2).map(|i| format!("{}/", parts[..i].join("/"))).collect()
}

// ---------------------------------------------------------------- status

pub fn status(root: &Path, scopes: &[String]) -> Result<Value, String> {
    let g = fresh_graph(root)?;
    let mut tot: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_dir: BTreeMap<String, [usize; 3]> = BTreeMap::new();
    let mut stale = Vec::new();
    let mut files_explained = 0;
    let mut files_in = 0;
    for f in g.files.iter().filter(|f| store::in_scope(&f.path, None, scopes)) {
        files_in += 1;
        let notes = store::load_notes(root, &f.path);
        if store::status_of(notes.file.as_ref(), &f.hash) == "ok" {
            files_explained += 1;
        }
        let dir = f.path.split('/').next().filter(|_| f.path.contains('/')).unwrap_or(".").to_string();
        for s in g.symbols.iter().filter(|s| s.file == f.path) {
            let st = store::status_of(notes.symbols.get(&s.id), &s.hash);
            *tot.entry(st).or_default() += 1;
            let e = by_dir.entry(dir.clone()).or_default();
            e[match st { "ok" => 0, "stale" => 1, _ => 2 }] += 1;
            if st == "stale" && stale.len() < 20 {
                stale.push(s.id.clone());
            }
        }
    }
    let total: usize = tot.values().sum();
    let ok = *tot.get("ok").unwrap_or(&0);
    Ok(json!({
        "ok": true, "files": files_in, "files_explained": files_explained, "symbols": total,
        "explained": ok, "stale": tot.get("stale").unwrap_or(&0), "missing": tot.get("missing").unwrap_or(&0),
        "coverage": if total == 0 { 1.0 } else { (ok as f64 / total as f64 * 1000.0).round() / 1000.0 },
        "by_dir": by_dir.iter().map(|(d, c)| json!({"dir": d, "ok": c[0], "stale": c[1], "missing": c[2]})).collect::<Vec<_>>(),
        "stale_sample": stale, "changed_files": git_changed(root).len(), "config": store::config(root),
    }))
}

// ---------------------------------------------------------------- pending

pub struct PendingOpts {
    pub scopes: Vec<String>,
    pub changed: bool,
    pub force: bool,
    pub budget: usize,
    pub limit: usize,
    pub max_lines: usize,
}

pub fn pending(root: &Path, o: &PendingOpts) -> Result<Value, String> {
    let g = fresh_graph(root)?;
    let changed: BTreeSet<String> = if o.changed { git_changed(root).into_iter().collect() } else { BTreeSet::new() };
    if o.changed && changed.is_empty() {
        return Ok(json!({"ok": true, "done": true, "items": [], "note": "no changed files in git status"}));
    }
    let names: HashMap<&str, &str> = g.symbols.iter().map(|s| (s.id.as_str(), s.name.as_str())).collect();
    let mut called_by: HashMap<&str, Vec<&str>> = HashMap::new();
    for s in &g.symbols {
        for c in &s.calls {
            called_by.entry(c.as_str()).or_default().push(s.name.as_str());
        }
    }
    let containers = ["class", "struct", "enum", "trait", "interface", "type"];
    let mut files_out = Vec::new();
    let (mut used_lines, mut count, mut remaining) = (0usize, 0usize, 0usize);
    let mut dirs_seen: BTreeSet<String> = BTreeSet::new();
    let dir_notes = store::load_dirs(root);
    for f in &g.files {
        if o.changed && !changed.contains(&f.path) {
            continue;
        }
        let file_scoped = store::in_scope(&f.path, None, &o.scopes);
        let syms: Vec<_> = g.symbols.iter().filter(|s| s.file == f.path && (file_scoped || store::in_scope(&s.file, Some(&s.id), &o.scopes))).collect();
        if syms.is_empty() && !file_scoped {
            continue;
        }
        let notes = store::load_notes(root, &f.path);
        let src = fs::read_to_string(root.join(&f.path)).unwrap_or_default();
        let lines: Vec<&str> = src.lines().collect();
        let mut items = Vec::new();
        for s in syms {
            let st = store::status_of(notes.symbols.get(&s.id), &s.hash);
            if st == "ok" && !o.force {
                continue;
            }
            let span = s.end.saturating_sub(s.start) + 1;
            let cap = if containers.contains(&s.kind.as_str()) { o.max_lines.min(25) } else { o.max_lines };
            let take = span.min(cap);
            if count >= o.limit || (used_lines + take > o.budget && count > 0) {
                remaining += 1;
                continue;
            }
            let mut code: String = lines.get(s.start - 1..(s.start - 1 + take).min(lines.len())).unwrap_or(&[]).join("\n");
            if span > take {
                code.push_str(&format!("\n… ({} more lines)", span - take));
            }
            used_lines += take;
            count += 1;
            let mut it = json!({
                "id": s.id, "kind": s.kind, "lines": format!("{}-{}", s.start, s.end), "status": st, "code": code,
                "calls": s.calls.iter().filter_map(|c| names.get(c.as_str())).collect::<Vec<_>>(),
                "called_by": called_by.get(s.id.as_str()).map(|v| v.iter().take(8).collect::<Vec<_>>()).unwrap_or_default(),
            });
            if st == "stale" {
                if let Some(n) = notes.symbols.get(&s.id) {
                    it["old_summary"] = json!(n.summary);
                }
            }
            items.push(it);
        }
        let fst = store::status_of(notes.file.as_ref(), &f.hash);
        if items.is_empty() && (fst == "ok" && !o.force || !file_scoped) {
            continue;
        }
        for d in top_dirs(&f.path) {
            if !dir_notes.contains_key(&d) {
                dirs_seen.insert(d);
            }
        }
        files_out.push(json!({
            "file": f.path, "lang": f.lang, "file_status": fst, "items": items,
            "all_symbols": g.symbols.iter().filter(|s| s.file == f.path).map(|s| s.name.as_str()).collect::<Vec<_>>(),
        }));
        if count >= o.limit {
            remaining += 1;
        }
    }
    let done = files_out.is_empty();
    Ok(json!({
        "ok": true, "done": done, "config": store::config(root), "symbols": count, "code_lines": used_lines,
        "more_pending": remaining, "files": files_out,
        "dirs_without_summary": dirs_seen.into_iter().take(10).collect::<Vec<_>>(),
        "write_with": "atlas write <file.json>  ·  [{\"id\":\"…\",\"summary\":\"…\",\"detail\":\"…\"}, {\"file\":\"…\",\"summary\":\"…\"}, {\"dir\":\"src/x/\",\"summary\":\"…\"}]",
    }))
}

// ---------------------------------------------------------------- write

pub fn write_items(root: &Path, items: &[Value]) -> Result<Value, String> {
    let g = fresh_graph(root)?;
    let syms: HashMap<&str, &parse::Symbol> = g.symbols.iter().map(|s| (s.id.as_str(), s)).collect();
    let files: HashMap<&str, &store::FileInfo> = g.files.iter().map(|f| (f.path.as_str(), f)).collect();
    let mut by_file: BTreeMap<String, FileNotes> = BTreeMap::new();
    let mut dirs = store::load_dirs(root);
    let (mut n_sym, mut n_file, mut n_dir) = (0, 0, 0);
    let mut unknown = Vec::new();
    let mut empty = Vec::new();
    let now = store::now();
    let mut problems: Vec<(String, check::Human)> = Vec::new();
    for it in items {
        if let Some(pid) = it["problem"].as_str() {
            let what = it["what"].as_str().or(it["summary"].as_str()).unwrap_or("").trim().to_string();
            if what.is_empty() {
                empty.push(pid.to_string());
                continue;
            }
            let tries = it["try"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            problems.push((pid.to_string(), check::Human {
                what, why: it["why"].as_str().unwrap_or("").trim().into(), fix: it["fix"].as_str().unwrap_or("").trim().into(), r#try: tries, at: 0,
            }));
            continue;
        }
        let summary = it["summary"].as_str().unwrap_or("").trim().to_string();
        let detail = it["detail"].as_str().unwrap_or("").trim().to_string();
        let key = it["id"].as_str().or(it["file"].as_str()).or(it["dir"].as_str()).unwrap_or("?").to_string();
        if summary.is_empty() {
            empty.push(key);
            continue;
        }
        if let Some(id) = it["id"].as_str() {
            let Some(s) = syms.get(id) else {
                unknown.push(id.to_string());
                continue;
            };
            let fnotes = by_file.entry(s.file.clone()).or_insert_with(|| store::load_notes(root, &s.file));
            fnotes.symbols.insert(id.to_string(), Note { summary, detail, hash: s.hash.clone(), at: now });
            n_sym += 1;
        } else if let Some(fp) = it["file"].as_str() {
            let Some(f) = files.get(fp) else {
                unknown.push(fp.to_string());
                continue;
            };
            let fnotes = by_file.entry(fp.to_string()).or_insert_with(|| store::load_notes(root, fp));
            fnotes.file = Some(Note { summary, detail, hash: f.hash.clone(), at: now });
            n_file += 1;
        } else if let Some(d) = it["dir"].as_str() {
            let d = if d.ends_with('/') || d.is_empty() { d.to_string() } else { format!("{d}/") };
            dirs.insert(d, Note { summary, detail, hash: String::new(), at: now });
            n_dir += 1;
        } else {
            unknown.push(key);
        }
    }
    for (f, n) in &by_file {
        store::save_notes(root, f, n)?;
    }
    if n_dir > 0 {
        store::save_dirs(root, &dirs)?;
    }
    let refs: Vec<(&str, check::Human)> = problems.iter().map(|(i, h)| (i.as_str(), h.clone())).collect();
    let (n_prob, unk) = if refs.is_empty() { (0, vec![]) } else { check::write_human(root, &refs) };
    unknown.extend(unk);
    Ok(json!({"ok": unknown.is_empty() && empty.is_empty(), "symbols": n_sym, "files": n_file, "dirs": n_dir, "problems": n_prob, "unknown": unknown, "empty_summary": empty}))
}

// ---------------------------------------------------------------- show

pub fn show(root: &Path, key: &str) -> Result<String, String> {
    let g = store::load_graph(root).map(Ok).unwrap_or_else(|| fresh_graph(root))?;
    let mut o = String::new();
    let file = if g.files.iter().any(|f| f.path == key) { Some(key.to_string()) } else { g.symbols.iter().find(|s| s.id == key).map(|s| s.file.clone()) };
    if let Some(file) = file {
        let notes = store::load_notes(root, &file);
        let fi = g.files.iter().find(|f| f.path == file).unwrap();
        if key == file {
            o.push_str(&format!("# {file}\n"));
            match &notes.file {
                Some(n) => o.push_str(&format!("{}{}\n{}\n", n.summary, if n.hash != fi.hash { "  [อาจล้าสมัย / may be outdated]" } else { "" }, n.detail)),
                None => o.push_str("(no explanation yet — run /atlas on this file)\n"),
            }
        }
        for s in g.symbols.iter().filter(|s| s.file == file && (key == file || s.id == key)) {
            o.push_str(&format!("\n## {} ({}, line {})\n", s.name, s.kind, s.start));
            match notes.symbols.get(&s.id) {
                Some(n) => {
                    o.push_str(&n.summary);
                    if n.hash != s.hash {
                        o.push_str("  [อาจล้าสมัย / may be outdated]");
                    }
                    o.push('\n');
                    if !n.detail.is_empty() {
                        o.push_str(&n.detail);
                        o.push('\n');
                    }
                }
                None => o.push_str("(no explanation yet)\n"),
            }
        }
        return Ok(o);
    }
    let dirs = store::load_dirs(root);
    let d = if key.ends_with('/') { key.to_string() } else { format!("{key}/") };
    if let Some(n) = dirs.get(&d) {
        return Ok(format!("# {d}\n{}\n{}\n", n.summary, n.detail));
    }
    Err(format!("not found: {key} (use a file path, symbol id like src/a.ts#fn, or folder)"))
}

// ---------------------------------------------------------------- vault (Obsidian export)

fn vault(root: &Path) -> Result<Value, String> {
    let g = fresh_graph(root)?;
    let vdir = store::atlas_dir(root).join("vault");
    let _ = fs::remove_dir_all(&vdir);
    let names: HashMap<&str, (&str, &str)> = g.symbols.iter().map(|s| (s.id.as_str(), (s.file.as_str(), s.name.as_str()))).collect();
    let mut called_by: HashMap<&str, Vec<&str>> = HashMap::new();
    for s in &g.symbols {
        for c in &s.calls {
            called_by.entry(c.as_str()).or_default().push(s.id.as_str());
        }
    }
    let link = |id: &str| names.get(id).map(|(f, n)| format!("[[{f}#{n}|{n}]]")).unwrap_or_default();
    let dirs = store::load_dirs(root);
    let mut index = String::from("# Project map\n\n");
    if let Some(n) = dirs.get("") {
        index.push_str(&format!("{}\n\n{}\n\n", n.summary, n.detail));
    }
    for f in &g.files {
        let notes = store::load_notes(root, &f.path);
        let mut md = format!("---\nsource: {}\nlang: {}\n---\n# {}\n\n", f.path, f.lang, f.path);
        if let Some(n) = &notes.file {
            md.push_str(&format!("{}\n\n{}\n\n", n.summary, n.detail));
        }
        for s in g.symbols.iter().filter(|s| s.file == f.path) {
            md.push_str(&format!("## {}\n\n*{} · line {}*\n\n", s.name, s.kind, s.start));
            if let Some(n) = notes.symbols.get(&s.id) {
                md.push_str(&format!("{}\n\n{}\n\n", n.summary, n.detail));
            }
            if !s.calls.is_empty() {
                md.push_str(&format!("Uses: {}\n\n", s.calls.iter().map(|c| link(c)).collect::<Vec<_>>().join(", ")));
            }
            if let Some(v) = called_by.get(s.id.as_str()) {
                md.push_str(&format!("Used by: {}\n\n", v.iter().map(|c| link(c)).collect::<Vec<_>>().join(", ")));
            }
        }
        let p = vdir.join(format!("{}.md", f.path));
        fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(&p, md).map_err(|e| e.to_string())?;
        let summary = notes.file.as_ref().map(|n| n.summary.as_str()).unwrap_or("");
        index.push_str(&format!("- [[{}]] {}\n", f.path, summary));
    }
    fs::write(vdir.join("index.md"), index).map_err(|e| e.to_string())?;
    Ok(json!({"ok": true, "vault": vdir.to_string_lossy(), "files": g.files.len(), "open_in_obsidian": "Obsidian → Open folder as vault → .atlas/vault"}))
}

// ---------------------------------------------------------------- cli

fn flag<'a>(args: &'a [String], n: &str) -> Option<&'a str> {
    args.iter().position(|a| a == n).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

fn positional(args: &[String]) -> Vec<String> {
    let with_val = ["--budget", "--limit", "--max-lines", "--lang", "--level", "--port", "--log", "--depth"];
    let mut v = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if with_val.contains(&args[i].as_str()) {
            i += 2;
            continue;
        }
        if !args[i].starts_with("--") {
            v.push(args[i].replace('\\', "/"));
        }
        i += 1;
    }
    v
}

const HELP: &str = r#"atlas — CodeAtlas: a local, agent-written, plain-language map of your codebase

usage: atlas [-C ROOT] <command> [args]

  init [--lang th|en] [--level beginner|dev]   set explanation language + depth (.atlas/config.json)
  scan                                         rebuild the structure (files, functions, calls) — no AI, no tokens
  status [scope...]                            coverage: explained / outdated / missing, by folder
  pending [scope...] [--changed] [--force] [--budget LINES] [--limit N] [--max-lines N]
                                               code that still needs explaining (scope = file, folder or symbol id)
  write <file.json | ->                        save explanations written by the agent
  show <file | symbol-id | folder/>            print explanations in the terminal
  inline [scope...] [--apply]                  write explanations into the source as comments (AST-checked; dry run by default)
  uninline [scope...] [--apply]                remove every [atlas] comment block
  check [scope...] [--tests]                   find errors: built-in syntax scan + the project's own checkers
                                               (tsc, python, ruff, cargo check, go vet; --tests also runs the test suite)
  debug -- <command ...>                       run a command, catch its errors / stack traces, map them to the code
  debug --log <file | ->                       same, from error output you copied (a log, a terminal paste)
  problems [scope...]                          read the problems in plain language, with commands to try yourself
  trace <symbol-id | name> [--depth 3]         who calls this, and what it calls (what breaks if it breaks)
  vault                                        export an Obsidian vault to .atlas/vault/
  open [--port 7420] [--no-browser]            local map app at http://127.0.0.1:7420
  mcp                                          MCP server (stdio) with the same commands as tools

nothing runs automatically: explanations are only written when you ask (e.g. /atlas src/payments)
exit codes: 0 ok · 1 failure · 2 nothing pending"#;

fn main() -> ExitCode {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let cwd = env::current_dir().unwrap();
    let mut root: PathBuf = cwd.clone();
    if args.first().map(|s| s.as_str()) == Some("-C") {
        if args.len() < 2 {
            return fail("-C needs a path");
        }
        root = cwd.join(&args[1]);
        args.drain(..2);
    }
    let Some(cmd) = args.first().cloned() else {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    };
    let rest = &args[1..];
    let res: Result<Value, String> = match cmd.as_str() {
        "init" => {
            let mut c = store::config(&root);
            if let Some(l) = flag(rest, "--lang") {
                c.lang = l.into();
            }
            if let Some(l) = flag(rest, "--level") {
                c.level = l.into();
            }
            store::save_config(&root, &c).and_then(|_| fresh_graph(&root)).map(|g| json!({"ok": true, "config": c, "files": g.files.len(), "symbols": g.symbols.len()}))
        }
        "scan" => fresh_graph(&root).map(|g| {
            let mut langs: BTreeMap<&str, usize> = BTreeMap::new();
            for f in &g.files {
                *langs.entry(&f.lang).or_default() += 1;
            }
            json!({"ok": true, "files": g.files.len(), "symbols": g.symbols.len(), "languages": langs,
                   "call_edges": g.symbols.iter().map(|s| s.calls.len()).sum::<usize>(), "unresolved_calls": g.unresolved_calls})
        }),
        "status" => status(&root, &positional(rest)),
        "pending" => {
            let o = PendingOpts {
                scopes: positional(rest),
                changed: rest.iter().any(|a| a == "--changed"),
                force: rest.iter().any(|a| a == "--force"),
                budget: flag(rest, "--budget").and_then(|v| v.parse().ok()).unwrap_or(500),
                limit: flag(rest, "--limit").and_then(|v| v.parse().ok()).unwrap_or(25),
                max_lines: flag(rest, "--max-lines").and_then(|v| v.parse().ok()).unwrap_or(60),
            };
            match pending(&root, &o) {
                Ok(v) => {
                    let done = v["done"].as_bool() == Some(true);
                    out(&v);
                    return if done { ExitCode::from(2) } else { ExitCode::SUCCESS };
                }
                Err(e) => Err(e),
            }
        }
        "write" => {
            let Some(src) = positional(rest).first().cloned() else { return fail("usage: atlas write <file.json | ->") };
            let text = if src == "-" {
                let mut s = String::new();
                let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut s);
                s
            } else {
                match fs::read_to_string(root.join(&src)).or_else(|_| fs::read_to_string(&src)) {
                    Ok(t) => t,
                    Err(e) => return fail(format!("{src}: {e}")),
                }
            };
            match serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) {
                Ok(Value::Array(a)) => write_items(&root, &a),
                Ok(v @ Value::Object(_)) => write_items(&root, &[v]),
                Ok(_) => Err("expected a JSON array of items".into()),
                Err(e) => Err(format!("bad JSON: {e}")),
            }
        }
        "show" => {
            let Some(k) = positional(rest).first().cloned() else { return fail("usage: atlas show <file|id|folder/>") };
            match show(&root, &k) {
                Ok(t) => {
                    println!("{t}");
                    return ExitCode::SUCCESS;
                }
                Err(e) => Err(e),
            }
        }
        "inline" => fresh_graph(&root).and_then(|g| inline::run(&root, &g, &positional(rest), rest.iter().any(|a| a == "--apply"), false)),
        "uninline" => fresh_graph(&root).and_then(|g| inline::run(&root, &g, &positional(rest), rest.iter().any(|a| a == "--apply"), true)),
        "vault" => vault(&root),
        "check" => match check::check(&root, &positional(rest), rest.iter().any(|a| a == "--tests")) {
            Ok(v) => {
                let ok = v["ok"].as_bool() == Some(true);
                out(&v);
                return if ok { ExitCode::SUCCESS } else { ExitCode::from(1) };
            }
            Err(e) => Err(e),
        },
        "debug" => {
            let cmd = rest.iter().position(|a| a == "--").map(|i| rest[i + 1..].join(" ")).filter(|c| !c.is_empty());
            let log = match flag(rest, "--log") {
                Some("-") => {
                    let mut s = String::new();
                    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut s);
                    Some(s)
                }
                Some(f) => match fs::read_to_string(root.join(f)).or_else(|_| fs::read_to_string(f)) {
                    Ok(t) => Some(t),
                    Err(e) => return fail(format!("{f}: {e}")),
                },
                None => None,
            };
            check::debug(&root, cmd.as_deref(), log, false)
        }
        "problems" => {
            println!("{}", check::problems_text(&root, &positional(rest)));
            return ExitCode::SUCCESS;
        }
        "trace" => {
            let Some(k) = positional(rest).first().cloned() else { return fail("usage: atlas trace <symbol-id | name> [--depth 3]") };
            let depth = flag(rest, "--depth").and_then(|d| d.parse().ok()).unwrap_or(3);
            match fresh_graph(&root).and_then(|g| check::trace_text(&root, &g, &k, depth)) {
                Ok(t) => {
                    println!("{t}");
                    return ExitCode::SUCCESS;
                }
                Err(e) => Err(e),
            }
        }
        "open" | "serve" => {
            let port = flag(rest, "--port").and_then(|p| p.parse().ok()).unwrap_or(7420);
            return match serve::run(&root, port, !rest.iter().any(|a| a == "--no-browser")) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => fail(e),
            };
        }
        "mcp" => {
            mcp::run(&root);
            return ExitCode::SUCCESS;
        }
        "-h" | "--help" | "help" => {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        "--version" | "-V" => {
            println!("atlas {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        _ => Err(format!("unknown command: {cmd} (atlas --help)")),
    };
    match res {
        Ok(v) => {
            let ok = v["ok"].as_bool() != Some(false);
            out(&v);
            if ok { ExitCode::SUCCESS } else { ExitCode::from(1) }
        }
        Err(e) => fail(e),
    }
}
