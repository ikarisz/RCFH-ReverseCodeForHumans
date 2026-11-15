//! Write explanations into the source as comments, without ever changing code.
//!
//! Safety: every edit only adds/removes `[atlas]` comment blocks. Before a file
//! is written, its comment-free AST fingerprint (tree-sitter) must be identical
//! to the original's and no new syntax errors may appear; otherwise the file is
//! left untouched and reported as unsafe. Dry run by default.

use crate::parse;
use crate::store::{self, Graph};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

const TAG: &str = "[atlas]";

fn line_prefix(lang: &str) -> &'static str {
    match lang {
        "python" => "#",
        "rust" => "///",
        _ => "//",
    }
}

fn uses_jsdoc(lang: &str) -> bool {
    matches!(lang, "javascript" | "typescript" | "tsx")
}

/// Remove every `[atlas]` block. Returns (new lines, blocks removed).
fn strip(lines: &[String], lang: &str) -> (Vec<String>, usize) {
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    let mut n = 0;
    let pre = line_prefix(lang);
    let cont = format!("{pre}   ");
    while i < lines.len() {
        let t = lines[i].trim();
        if uses_jsdoc(lang) && t == "/**" && lines.get(i + 1).is_some_and(|l| l.trim().starts_with(&format!("* {TAG}"))) {
            let mut j = i + 1;
            while j < lines.len() && !lines[j].trim().ends_with("*/") {
                j += 1;
            }
            i = j + 1;
            let at_top = out.iter().all(|l: &String| l.starts_with("#!"));
            if at_top && lines.get(i).is_some_and(|l| l.trim().is_empty()) {
                i += 1;
            }
            n += 1;
            continue;
        }
        let file_pre = if lang == "rust" { "//" } else { pre };
        if t.starts_with(&format!("{pre} {TAG}")) || t.starts_with(&format!("{file_pre} {TAG}")) {
            let p = if t.starts_with(&format!("{pre} {TAG}")) { pre } else { file_pre };
            let c = format!("{p}   ");
            i += 1;
            while i < lines.len() && (lines[i].trim_start().starts_with(&c) || lines[i].trim() == p) {
                i += 1;
            }
            // a file-level block is followed by one blank line we added
            let at_top = out.iter().all(|l: &String| l.starts_with("#!") || (lang == "python" && l.starts_with('#') && (l.contains("coding") || l.contains("vim:"))));
            if at_top && lines.get(i).is_some_and(|l| l.trim().is_empty()) {
                i += 1;
            }
            n += 1;
            continue;
        }
        let _ = &cont;
        out.push(lines[i].clone());
        i += 1;
    }
    (out, n)
}

fn block(lang: &str, indent: &str, summary: &str, detail: &str, file_level: bool, eol_cr: bool) -> Vec<String> {
    let cr = if eol_cr { "\r" } else { "" };
    let mut body: Vec<String> = vec![];
    let detail_lines: Vec<&str> = detail.lines().map(|l| l.trim_end()).collect();
    if uses_jsdoc(lang) {
        let clean = |s: &str| s.replace("*/", "*\\/");
        body.push(format!("{indent}/**{cr}"));
        body.push(format!("{indent} * {TAG} {}{cr}", clean(summary.trim())));
        if !detail_lines.is_empty() {
            body.push(format!("{indent} *{cr}"));
            for l in detail_lines {
                body.push(if l.is_empty() { format!("{indent} *{cr}") } else { format!("{indent} * {}{cr}", clean(l)) });
            }
        }
        body.push(format!("{indent} */{cr}"));
    } else {
        let pre = if file_level && lang == "rust" { "//" } else { line_prefix(lang) };
        body.push(format!("{indent}{pre} {TAG} {}{cr}", summary.trim().replace('\n', " ")));
        for l in detail_lines {
            body.push(if l.is_empty() { format!("{indent}{pre}{cr}") } else { format!("{indent}{pre}   {l}{cr}") });
        }
    }
    body
}

fn split_lines(src: &str) -> (Vec<String>, bool, bool) {
    let cr = src.contains("\r\n");
    let trailing_nl = src.ends_with('\n');
    let body = if trailing_nl { &src[..src.len() - 1] } else { src };
    (body.split('\n').map(|s| s.trim_end_matches('\r').to_string()).collect(), cr, trailing_nl)
}

fn join_lines(lines: &[String], cr: bool, trailing: bool) -> String {
    let eol = if cr { "\r\n" } else { "\n" };
    let mut s: String = lines.iter().map(|l| l.trim_end_matches('\r')).collect::<Vec<_>>().join(eol);
    if trailing {
        s.push_str(eol);
    }
    s
}

pub fn run(root: &Path, g: &Graph, scopes: &[String], apply: bool, remove_only: bool) -> Result<Value, String> {
    let mut report = Vec::new();
    let (mut files_changed, mut blocks_added, mut blocks_removed, mut unsafe_files, mut skipped_stale) = (0, 0, 0, 0, 0);
    for f in g.files.iter().filter(|f| store::in_scope(&f.path, None, scopes) || g.symbols.iter().any(|s| s.file == f.path && store::in_scope(&s.file, Some(&s.id), scopes))) {
        let path = root.join(&f.path);
        let Ok(orig) = fs::read_to_string(&path) else { continue };
        let notes = store::load_notes(root, &f.path);
        if !remove_only && notes.symbols.is_empty() && notes.file.is_none() {
            continue;
        }
        let Some(fp0) = parse::fingerprint(&f.lang, &orig) else { continue };
        let (lines, cr, trailing) = split_lines(&orig);
        let (mut lines, removed) = strip(&lines, &f.lang);
        let mut added = 0;
        if !remove_only {
            let stripped = join_lines(&lines, cr, trailing);
            let syms = parse::extract(&f.path, &f.lang, &stripped);
            let only_ids = |id: &str| store::in_scope(&f.path, Some(id), scopes);
            let mut inserts: Vec<(usize, Vec<String>)> = Vec::new();
            let mut used_lines = std::collections::BTreeSet::new();
            for s in &syms {
                if !only_ids(&s.id) {
                    continue;
                }
                let Some(n) = notes.symbols.get(&s.id) else { continue };
                if n.hash != s.hash {
                    skipped_stale += 1;
                    continue;
                }
                if !used_lines.insert(s.anchor) {
                    continue;
                }
                let line = &lines[s.anchor - 1];
                let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
                inserts.push((s.anchor - 1, block(&f.lang, &indent, &n.summary, &n.detail, false, false)));
            }
            if let Some(n) = notes.file.as_ref().filter(|n| n.hash == f.hash && store::in_scope(&f.path, None, scopes)) {
                let mut at = 0;
                while at < lines.len() && (lines[at].starts_with("#!") || (f.lang == "python" && lines[at].starts_with('#') && (lines[at].contains("coding") || lines[at].contains("vim:")))) {
                    at += 1;
                }
                let mut b = block(&f.lang, "", &n.summary, &n.detail, true, false);
                b.push(String::new());
                inserts.push((at, b));
            }
            inserts.sort_by(|a, b| b.0.cmp(&a.0));
            for (at, b) in inserts {
                added += 1;
                let tail = lines.split_off(at);
                lines.extend(b);
                lines.extend(tail);
            }
        }
        let new = join_lines(&lines, cr, trailing);
        if new == orig {
            continue;
        }
        let fp1 = parse::fingerprint(&f.lang, &new);
        let safe = fp1.as_ref().is_some_and(|x| x.0 == fp0.0 && (!x.1 || fp0.1));
        if !safe {
            unsafe_files += 1;
            report.push(json!({"file": f.path, "safe": false, "note": "code structure would change — file left untouched"}));
            continue;
        }
        files_changed += 1;
        blocks_added += added;
        blocks_removed += removed;
        if apply {
            fs::write(&path, &new).map_err(|e| format!("{}: {e}", f.path))?;
        }
        if report.len() < 40 {
            report.push(json!({"file": f.path, "safe": true, "added": added, "removed": removed}));
        }
    }
    Ok(json!({
        "ok": unsafe_files == 0, "applied": apply, "files": files_changed, "blocks_added": blocks_added, "blocks_removed": blocks_removed,
        "skipped_outdated": skipped_stale, "unsafe_files": unsafe_files, "report": report,
        "next": if apply { "done — review with `git diff`; undo with `atlas uninline <scope> --apply`" } else { "dry run — add --apply to write" },
    }))
}
