//! Error checking and debugging, on demand.
//!
//! `atlas check` runs the project's own checkers (whatever is installed) plus a
//! built-in tree-sitter syntax scan, and `atlas debug` runs any command (or reads
//! a pasted log) and pulls out errors and stack traces. Every problem is mapped
//! to the function that contains it, with a code excerpt, who is affected
//! (callers) and a command the user can re-run themselves. The agent then adds
//! a plain-language explanation (`atlas write` with `problem` items).

use crate::parse;
use crate::store::{self, Graph};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::Instant;
use std::{fs, io::Write};

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Human {
    pub what: String,
    #[serde(default)]
    pub why: String,
    #[serde(default)]
    pub fix: String,
    #[serde(default)]
    pub r#try: Vec<String>,
    #[serde(default)]
    pub at: u64,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Frame {
    pub file: String,
    pub line: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Problem {
    pub id: String,
    /// "check" (static) or "debug" (from running a command / a log)
    pub source: String,
    pub tool: String,
    pub severity: String,
    pub file: String,
    pub line: usize,
    #[serde(default)]
    pub col: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub affects: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<Frame>,
    #[serde(default)]
    pub snippet: String,
    /// commands the user can run themselves to see / re-check this problem
    #[serde(default)]
    pub rerun: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human: Option<Human>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Run {
    pub tool: String,
    pub cmd: String,
    pub ok: bool,
    #[serde(default)]
    pub exit: Option<i32>,
    #[serde(default)]
    pub ms: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub skipped: String,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Report {
    pub at: u64,
    pub runs: Vec<Run>,
    pub problems: Vec<Problem>,
}

fn report_path(root: &Path) -> PathBuf {
    store::atlas_dir(root).join("problems.json")
}

pub fn load(root: &Path) -> Report {
    fs::read_to_string(report_path(root)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(root: &Path, r: &Report) -> Result<(), String> {
    fs::create_dir_all(store::atlas_dir(root)).map_err(|e| e.to_string())?;
    fs::write(report_path(root), serde_json::to_string_pretty(r).unwrap()).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- helpers

fn shell_cmd(cmd: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(cmd);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    }
}

fn has_tool(cmd: &str) -> bool {
    let probe = if cmd == "go" { "go version".to_string() } else { format!("{cmd} --version") };
    shell_cmd(&probe).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn run_capture(cmd: &str, cwd: &Path, stdin: Option<&str>) -> (Option<i32>, String, u64) {
    run_with(shell_cmd(cmd), cmd, cwd, stdin)
}

/// Run a program directly (no shell), so paths with spaces/quotes survive on Windows.
fn run_argv(argv: &[&str], cwd: &Path, stdin: Option<&str>) -> (Option<i32>, String, u64) {
    let mut c = Command::new(argv[0]);
    c.args(&argv[1..]);
    run_with(c, argv[0], cwd, stdin)
}

fn run_with(mut c: Command, cmd: &str, cwd: &Path, stdin: Option<&str>) -> (Option<i32>, String, u64) {
    let t = Instant::now();
    c.current_dir(cwd).stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    let Ok(mut child) = c.spawn() else { return (None, format!("could not start: {cmd}"), 0) };
    if let (Some(s), Some(mut si)) = (stdin, child.stdin.take()) {
        let _ = si.write_all(s.as_bytes());
    }
    match child.wait_with_output() {
        Ok(o) => (o.status.code(), format!("{}\n{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)), t.elapsed().as_millis() as u64),
        Err(e) => (None, e.to_string(), t.elapsed().as_millis() as u64),
    }
}

fn rel_to_root(root: &Path, cwd: &Path, p: &str) -> String {
    let p = p.trim().trim_start_matches("./").replace('\\', "/");
    let abs = if Path::new(&p).is_absolute() { PathBuf::from(&p) } else { cwd.join(&p) };
    let root_c = root.canonicalize().unwrap_or(root.to_path_buf());
    let abs_c = abs.canonicalize().unwrap_or(abs.clone());
    match abs_c.strip_prefix(&root_c) {
        Ok(r) => r.to_string_lossy().replace('\\', "/"),
        Err(_) => p,
    }
}

fn q(s: &str) -> String {
    if s.contains(' ') { format!("\"{s}\"") } else { s.to_string() }
}

/// Innermost symbol containing `line`.
fn symbol_at<'a>(g: &'a Graph, file: &str, line: usize) -> Option<&'a parse::Symbol> {
    g.symbols.iter().filter(|s| s.file == file && s.start <= line && line <= s.end).min_by_key(|s| s.end - s.start)
}

fn snippet(root: &Path, file: &str, line: usize) -> String {
    let Ok(t) = fs::read_to_string(root.join(file)) else { return String::new() };
    let lines: Vec<&str> = t.lines().collect();
    if line == 0 || line > lines.len() {
        return String::new();
    }
    let from = line.saturating_sub(3).max(1);
    let to = (line + 2).min(lines.len());
    (from..=to).map(|n| format!("{} {:>4} | {}", if n == line { ">" } else { " " }, n, lines[n - 1])).collect::<Vec<_>>().join("\n")
}

fn finish(root: &Path, g: &Graph, mut p: Problem, called_by: &HashMap<&str, Vec<&str>>, names: &HashMap<&str, &str>) -> Problem {
    if let Some(s) = symbol_at(g, &p.file, p.line) {
        p.symbol = Some(s.id.clone());
        p.affects = called_by.get(s.id.as_str()).map(|v| v.iter().take(8).map(|i| names.get(i).copied().unwrap_or(i).to_string()).collect()).unwrap_or_default();
    }
    for f in p.frames.iter_mut() {
        f.symbol = symbol_at(g, &f.file, f.line).map(|s| s.id.clone());
    }
    p.snippet = snippet(root, &p.file, p.line);
    p.id = parse::fnv(&format!("{}|{}|{}|{}", p.tool, p.file, p.line, p.message))[..10].to_string();
    p
}

fn indexes(g: &Graph) -> (HashMap<&str, Vec<&str>>, HashMap<&str, &str>) {
    let mut called_by: HashMap<&str, Vec<&str>> = HashMap::new();
    for s in &g.symbols {
        for c in &s.calls {
            called_by.entry(c.as_str()).or_default().push(s.id.as_str());
        }
    }
    let names = g.symbols.iter().map(|s| (s.id.as_str(), s.name.as_str())).collect();
    (called_by, names)
}

fn merge_and_save(root: &Path, source: &str, runs: Vec<Run>, mut fresh: Vec<Problem>, scopes: &[String]) -> Result<Report, String> {
    let old = load(root);
    let human: HashMap<String, Human> = old.problems.iter().filter_map(|p| p.human.clone().map(|h| (p.id.clone(), h))).collect();
    for p in fresh.iter_mut() {
        if p.human.is_none() {
            p.human = human.get(&p.id).cloned();
        }
    }
    // keep problems from the other source, and same-source problems outside the scope
    let mut keep: Vec<Problem> = old.problems.into_iter().filter(|p| p.source != source || (!scopes.is_empty() && !store::in_scope(&p.file, None, scopes))).collect();
    keep.extend(fresh);
    keep.sort_by(|a, b| (a.severity != "error", &a.file, a.line).cmp(&(b.severity != "error", &b.file, b.line)));
    keep.truncate(300);
    let r = Report { at: store::now(), runs, problems: keep };
    save(root, &r)?;
    Ok(r)
}

// ---------------------------------------------------------------- built-in syntax scan

fn syntax_scan(root: &Path, g: &Graph, scopes: &[String]) -> Vec<Problem> {
    let mut out = Vec::new();
    for f in g.files.iter().filter(|f| store::in_scope(&f.path, None, scopes)) {
        let Ok(src) = fs::read_to_string(root.join(&f.path)) else { continue };
        let Some(tree) = parse::parse(&f.lang, &src) else { continue };
        if !tree.root_node().has_error() {
            continue;
        }
        let mut stack = vec![tree.root_node()];
        let mut seen = BTreeSet::new();
        while let Some(n) = stack.pop() {
            if n.is_error() || n.is_missing() {
                let line = n.start_position().row + 1;
                let txt = n.utf8_text(src.as_bytes()).unwrap_or("").trim_start();
                if matches!(f.lang.as_str(), "tsx" | "javascript") && txt.starts_with('&') {
                    continue;
                }
                if seen.insert(line) && seen.len() <= 5 {
                    let near: String = n.utf8_text(src.as_bytes()).unwrap_or("").chars().take(40).collect();
                    let message = if n.is_missing() {
                        format!("missing `{}`", n.kind())
                    } else {
                        format!("syntax error near `{}`", near.split_whitespace().collect::<Vec<_>>().join(" "))
                    };
                    out.push(Problem {
                        source: "check".into(), tool: "syntax".into(), severity: "error".into(), file: f.path.clone(), line,
                        col: n.start_position().column + 1, message, rerun: vec![format!("atlas check {}", q(&f.path))], ..Default::default()
                    });
                }
                continue;
            }
            let mut c = n.walk();
            for ch in n.children(&mut c) {
                if ch.has_error() || ch.is_error() || ch.is_missing() {
                    stack.push(ch);
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------- checkers

static RE_TSC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.+?)\((\d+),(\d+)\): (error|warning) (TS\d+): (.*)$").unwrap());
static RE_CARGO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.+?\.rs):(\d+):(\d+): (error|warning)(?:\[(\w+)\])?: (.*)$").unwrap());
static RE_COLON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.+?\.(?:py|go|ts|tsx|js|jsx|rs)):(\d+):(\d+): (?:([A-Z]+\d+) )?(.*)$").unwrap());
const PY_SYNTAX: &str = r#"import sys, json
for f in json.load(sys.stdin):
    try:
        compile(open(f, encoding="utf-8", errors="replace").read(), f, "exec", dont_inherit=True)
    except SyntaxError as e:
        print(json.dumps({"file": f, "line": e.lineno or 1, "col": e.offset or 0, "msg": type(e).__name__ + ": " + str(e.msg)}))
    except Exception:
        pass
"#;

fn find_up(start: &Path, root: &Path, rel: &str) -> bool {
    let mut d = Some(start);
    while let Some(x) = d {
        if x.join(rel).exists() {
            return true;
        }
        if x == root {
            break;
        }
        d = x.parent();
    }
    false
}

fn project_dirs(root: &Path, marker: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = store::walk(root).into_iter().filter(|p| p.file_name().is_some_and(|n| n == marker)).filter_map(|p| p.parent().map(Path::to_path_buf)).collect();
    v.sort();
    // outermost only (workspaces handle members)
    let mut out: Vec<PathBuf> = Vec::new();
    for d in v {
        if !out.iter().any(|o| d.starts_with(o)) {
            out.push(d);
        }
    }
    out
}

pub fn check(root: &Path, scopes: &[String], tests: bool) -> Result<Value, String> {
    let g = crate::fresh_graph(root)?;
    let (called_by, names) = indexes(&g);
    let mut runs = Vec::new();
    let mut raw: Vec<Problem> = syntax_scan(root, &g, scopes);
    runs.push(Run { tool: "syntax".into(), cmd: "built-in (tree-sitter)".into(), ok: raw.is_empty(), ..Default::default() });
    let langs: BTreeSet<&str> = g.files.iter().filter(|f| store::in_scope(&f.path, None, scopes)).map(|f| f.lang.as_str()).collect();
    let rel = |p: &Path| store::rel(root, p);
    // (language family, dir) checked by the language's own compiler/checker: its verdict wins over the built-in scan
    let mut covered: Vec<(&str, String)> = Vec::new();

    // TypeScript
    if langs.iter().any(|l| matches!(*l, "typescript" | "tsx" | "javascript")) {
        for dir in project_dirs(root, "tsconfig.json") {
            let r = rel(&dir);
            let cmd = "npx --no-install tsc --noEmit --pretty false".to_string();
            let user_cmd = if r.is_empty() { cmd.clone() } else { format!("cd {} && {cmd}", q(&r)) };
            if !find_up(&dir, root, "node_modules/typescript") {
                runs.push(Run { tool: "tsc".into(), cmd: user_cmd, skipped: "TypeScript is not installed here — run `npm install` first".into(), ..Default::default() });
                continue;
            }
            let (code, out, ms) = run_capture(&cmd, &dir, None);
            if code.is_some() {
                covered.push(("js", r.clone()));
            }
            for l in out.lines() {
                if let Some(c) = RE_TSC.captures(l.trim()) {
                    raw.push(Problem { source: "check".into(), tool: "tsc".into(), severity: c[4].into(), file: rel_to_root(root, &dir, &c[1]), line: c[2].parse().unwrap_or(1), col: c[3].parse().unwrap_or(0), code: c[5].into(), message: c[6].into(), rerun: vec![user_cmd.clone()], ..Default::default() });
                }
            }
            runs.push(Run { tool: "tsc".into(), cmd: user_cmd, ok: code == Some(0), exit: code, ms, skipped: String::new() });
        }
    }
    // Python
    if langs.contains("python") {
        let py = if has_tool("python") { "python" } else if has_tool("python3") { "python3" } else { "" };
        let files: Vec<&str> = g.files.iter().filter(|f| f.lang == "python" && store::in_scope(&f.path, None, scopes)).map(|f| f.path.as_str()).collect();
        if py.is_empty() {
            runs.push(Run { tool: "python".into(), cmd: "python -m py_compile <file>".into(), skipped: "Python is not installed / not on PATH".into(), ..Default::default() });
        } else {
            let sp = std::env::temp_dir().join("atlas_py_syntax.py");
            let _ = fs::write(&sp, PY_SYNTAX);
            let sps = sp.to_string_lossy().to_string();
            let (code, out, ms) = run_argv(&[py, "-I", &sps], root, Some(&serde_json::to_string(&files).unwrap()));
            if code == Some(0) {
                covered.push(("python", String::new()));
            }
            let mut n = 0;
            for l in out.lines() {
                if let Ok(v) = serde_json::from_str::<Value>(l.trim()) {
                    let f = v["file"].as_str().unwrap_or("").to_string();
                    raw.push(Problem { source: "check".into(), tool: "python".into(), severity: "error".into(), line: v["line"].as_u64().unwrap_or(1) as usize, col: v["col"].as_u64().unwrap_or(0) as usize, message: v["msg"].as_str().unwrap_or("").into(), rerun: vec![format!("{py} -m py_compile {}", q(&f))], file: f, ..Default::default() });
                    n += 1;
                }
            }
            runs.push(Run { tool: "python".into(), cmd: format!("{py} -m py_compile <each .py file>"), ok: code == Some(0) && n == 0, exit: code, ms, skipped: String::new() });
            if has_tool("ruff") {
                let cmd = "ruff check --output-format=concise --quiet .".to_string();
                let (code, out, ms) = run_capture(&cmd, root, None);
                for l in out.lines() {
                    if let Some(c) = RE_COLON.captures(l.trim()) {
                        let f = rel_to_root(root, root, &c[1]);
                        if store::in_scope(&f, None, scopes) {
                            raw.push(Problem { source: "check".into(), tool: "ruff".into(), severity: if c.get(4).is_some_and(|m| m.as_str().starts_with('E') || m.as_str().starts_with('F')) { "error".into() } else { "warning".into() }, line: c[2].parse().unwrap_or(1), col: c[3].parse().unwrap_or(0), code: c.get(4).map(|m| m.as_str().into()).unwrap_or_default(), message: c[5].into(), rerun: vec![format!("ruff check {}", q(&f))], file: f, ..Default::default() });
                        }
                    }
                }
                runs.push(Run { tool: "ruff".into(), cmd, ok: code == Some(0), exit: code, ms, skipped: String::new() });
            }
        }
    }
    // Rust
    if langs.contains("rust") {
        for dir in project_dirs(root, "Cargo.toml") {
            let r = rel(&dir);
            let cmd = "cargo check --message-format=short --quiet".to_string();
            let user_cmd = if r.is_empty() { "cargo check".into() } else { format!("cd {} && cargo check", q(&r)) };
            if !has_tool("cargo") {
                runs.push(Run { tool: "cargo".into(), cmd: user_cmd, skipped: "Rust (cargo) is not installed".into(), ..Default::default() });
                break;
            }
            let (code, out, ms) = run_capture(&cmd, &dir, None);
            if code.is_some() {
                covered.push(("rust", r.clone()));
            }
            for l in out.lines() {
                if let Some(c) = RE_CARGO.captures(l.trim()) {
                    raw.push(Problem { source: "check".into(), tool: "cargo".into(), severity: c[4].into(), file: rel_to_root(root, &dir, &c[1]), line: c[2].parse().unwrap_or(1), col: c[3].parse().unwrap_or(0), code: c.get(5).map(|m| m.as_str().into()).unwrap_or_default(), message: c[6].into(), rerun: vec![user_cmd.clone()], ..Default::default() });
                }
            }
            runs.push(Run { tool: "cargo".into(), cmd: user_cmd, ok: code == Some(0), exit: code, ms, skipped: String::new() });
        }
    }
    // Go
    if langs.contains("go") {
        for dir in project_dirs(root, "go.mod") {
            let r = rel(&dir);
            let cmd = "go vet ./...".to_string();
            let user_cmd = if r.is_empty() { cmd.clone() } else { format!("cd {} && {cmd}", q(&r)) };
            if !has_tool("go") {
                runs.push(Run { tool: "go".into(), cmd: user_cmd, skipped: "Go is not installed".into(), ..Default::default() });
                break;
            }
            let (code, out, ms) = run_capture(&cmd, &dir, None);
            if code.is_some() {
                covered.push(("go", r.clone()));
            }
            for l in out.lines() {
                if let Some(c) = RE_COLON.captures(l.trim()) {
                    raw.push(Problem { source: "check".into(), tool: "go vet".into(), severity: "error".into(), file: rel_to_root(root, &dir, &c[1]), line: c[2].parse().unwrap_or(1), col: c[3].parse().unwrap_or(0), message: c[5].into(), rerun: vec![user_cmd.clone()], ..Default::default() });
                }
            }
            runs.push(Run { tool: "go vet".into(), cmd: user_cmd, ok: code == Some(0), exit: code, ms, skipped: String::new() });
        }
    }
    // Tests (opt-in): run them, parse failures like a debug run
    if tests {
        let mut test_cmds: Vec<(String, PathBuf)> = Vec::new();
        for dir in project_dirs(root, "package.json") {
            if fs::read_to_string(dir.join("package.json")).unwrap_or_default().contains("\"test\"") {
                test_cmds.push(("npm test --silent".into(), dir));
            }
        }
        for dir in project_dirs(root, "Cargo.toml") {
            test_cmds.push(("cargo test --quiet".into(), dir));
        }
        for dir in project_dirs(root, "go.mod") {
            test_cmds.push(("go test ./...".into(), dir));
        }
        if langs.contains("python") && has_tool("pytest") {
            test_cmds.push(("pytest -q".into(), root.to_path_buf()));
        }
        for (cmd, dir) in test_cmds {
            let r = rel(&dir);
            let user_cmd = if r.is_empty() { cmd.clone() } else { format!("cd {} && {cmd}", q(&r)) };
            let (code, out, ms) = run_capture(&cmd, &dir, None);
            if code != Some(0) {
                let mut ps = extract(root, &dir, &out, "test", &user_cmd, &g);
                for p in ps.iter_mut() {
                    p.source = "check".into();
                }
                raw.extend(ps);
            }
            runs.push(Run { tool: "test".into(), cmd: user_cmd, ok: code == Some(0), exit: code, ms, skipped: String::new() });
        }
    }
    // scope filter, drop built-in syntax hits where the real checker covered the file, dedupe, finish
    let family = |lang: &str| match lang {
        "javascript" | "typescript" | "tsx" => "js",
        "python" => "python",
        "rust" => "rust",
        _ => "go",
    };
    let lang_of: HashMap<&str, &str> = g.files.iter().map(|f| (f.path.as_str(), f.lang.as_str())).collect();
    let is_covered = |file: &str| -> bool {
        let Some(l) = lang_of.get(file) else { return false };
        covered.iter().any(|(fam, dir)| *fam == family(l) && (dir.is_empty() || file.starts_with(&format!("{dir}/"))))
    };
    let mut seen = BTreeSet::new();
    let mut problems: Vec<Problem> = raw
        .into_iter()
        .filter(|p| p.tool != "syntax" || !is_covered(&p.file))
        .filter(|p| store::in_scope(&p.file, None, scopes))
        .filter(|p| seen.insert((p.file.clone(), p.line, p.message.clone())))
        .map(|mut p| {
            let again = format!("atlas check {}", q(&p.file));
            if !p.rerun.contains(&again) {
                p.rerun.push(again);
            }
            finish(root, &g, p, &called_by, &names)
        })
        .collect();
    // syntax scan duplicates a language checker's error on the same line → keep the checker's
    let checker_lines: BTreeSet<(String, usize)> = problems.iter().filter(|p| p.tool != "syntax").map(|p| (p.file.clone(), p.line)).collect();
    problems.retain(|p| p.tool != "syntax" || !checker_lines.contains(&(p.file.clone(), p.line)));
    let r = merge_and_save(root, "check", runs, problems, scopes)?;
    Ok(summary(root, &r, "check"))
}

// ---------------------------------------------------------------- debug: run a command / read a log

static RE_PY_FRAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"File "(.+?)", line (\d+)"#).unwrap());
static RE_JS_FRAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"at (?:.+? \()?(?:file:///)?(.+?\.(?:[cm]?[jt]sx?)):(\d+):(\d+)\)?").unwrap());
static RE_RS_FRAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:-->|at) (.+?\.rs):(\d+)(?::(\d+))?").unwrap());
static RE_ANY_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([\w./\\-]+\.(?:py|go|rs|[cm]?[jt]sx?))[:(](\d+)(?:[:,](\d+))?").unwrap());
static RE_ERRLINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*(?:uncaught |unhandled )?(?:[\w.]*(?:error|exception)\b|panic(?:ked)?\b|thread '.+' panicked|fatal|assert(?:ion)?\w*|traceback|failed|fail:|✗|×)").unwrap());

fn extract(root: &Path, cwd: &Path, out: &str, tool: &str, user_cmd: &str, g: &Graph) -> Vec<Problem> {
    let files: BTreeSet<&str> = g.files.iter().map(|f| f.path.as_str()).collect();
    let lines: Vec<&str> = out.lines().collect();
    let refs_in = |l: &str| -> Vec<Frame> {
        let mut v = Vec::new();
        for re in [&*RE_PY_FRAME, &*RE_JS_FRAME, &*RE_RS_FRAME, &*RE_ANY_REF] {
            for c in re.captures_iter(l) {
                let f = rel_to_root(root, cwd, &c[1]);
                if files.contains(f.as_str()) {
                    let fr = Frame { file: f, line: c[2].parse().unwrap_or(1), symbol: None };
                    if !v.iter().any(|x: &Frame| x.file == fr.file && x.line == fr.line) {
                        v.push(fr);
                    }
                }
            }
            if !v.is_empty() {
                break;
            }
        }
        v
    };
    let mut out_p = Vec::new();
    let mut used_msgs = BTreeSet::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        // Python traceback block: frames then the message line
        if l.trim_start().starts_with("Traceback (most recent call last)") {
            let mut frames = Vec::new();
            let mut j = i + 1;
            while j < lines.len() && (lines[j].starts_with(' ') || lines[j].trim().is_empty()) {
                frames.extend(refs_in(lines[j]));
                j += 1;
            }
            frames.dedup_by(|a, b| a.file == b.file && a.line == b.line);
            let msg = lines.get(j).map(|s| s.trim().to_string()).unwrap_or_else(|| "Python error".into());
            if let Some(loc) = frames.last().cloned() {
                if used_msgs.insert(msg.clone()) {
                    out_p.push(Problem { source: "debug".into(), tool: tool.into(), severity: "error".into(), file: loc.file.clone(), line: loc.line, message: msg, frames: frames.into_iter().rev().collect(), rerun: vec![user_cmd.to_string()], ..Default::default() });
                }
            }
            i = j + 1;
            continue;
        }
        if RE_ERRLINE.is_match(l) && !l.trim_start().starts_with("at ") {
            let msg: String = l.trim().chars().take(300).collect();
            // frames: refs on this line, then the following stack lines, then a few lines before
            let mut frames = refs_in(l);
            for k in i + 1..(i + 16).min(lines.len()) {
                if RE_ERRLINE.is_match(lines[k]) && !lines[k].trim_start().starts_with("at ") {
                    break;
                }
                frames.extend(refs_in(lines[k]));
            }
            if frames.is_empty() {
                for k in i.saturating_sub(3)..i {
                    frames.extend(refs_in(lines[k]));
                }
            }
            frames.dedup_by(|a, b| a.file == b.file && a.line == b.line);
            if let Some(loc) = frames.first().cloned() {
                if used_msgs.insert(msg.clone()) {
                    out_p.push(Problem { source: "debug".into(), tool: tool.into(), severity: "error".into(), file: loc.file.clone(), line: loc.line, message: msg, frames: frames.into_iter().take(8).collect(), rerun: vec![user_cmd.to_string()], ..Default::default() });
                }
            }
        }
        i += 1;
    }
    out_p.truncate(30);
    out_p
}

/// Run `cmd` (or read `log`), capture errors and map them onto the code.
pub fn debug(root: &Path, cmd: Option<&str>, log: Option<String>, timeout_note: bool) -> Result<Value, String> {
    let g = crate::fresh_graph(root)?;
    let (called_by, names) = indexes(&g);
    let (output, user_cmd, run) = match (cmd, log) {
        (Some(c), _) => {
            let (code, out, ms) = run_capture(c, root, None);
            (out, c.to_string(), Some(Run { tool: "debug".into(), cmd: c.into(), ok: code == Some(0), exit: code, ms, skipped: String::new() }))
        }
        (None, Some(l)) => (l, "(pasted output)".to_string(), None),
        _ => return Err("usage: atlas debug -- <command>   or   atlas debug --log <file|->".into()),
    };
    let _ = timeout_note;
    let problems: Vec<Problem> = extract(root, root, &output, "debug", &user_cmd, &g)
        .into_iter()
        .map(|mut p| {
            if user_cmd.starts_with('(') {
                p.rerun.clear();
            }
            finish(root, &g, p, &called_by, &names)
        })
        .collect();
    let exit_failed = run.as_ref().is_some_and(|r| !r.ok);
    let tail: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = tail[tail.len().saturating_sub(25)..].join("\n");
    let r = merge_and_save(root, "debug", run.clone().into_iter().collect(), problems, &[])?;
    let mut v = summary(root, &r, "debug");
    v["command"] = json!(user_cmd);
    v["exit"] = json!(run.as_ref().and_then(|r| r.exit));
    v["command_failed"] = json!(exit_failed);
    // when nothing maps to project code, still hand over the raw tail so it can be explained
    if r.problems.iter().all(|p| p.source != "debug") {
        v["output_tail"] = json!(tail);
    }
    Ok(v)
}

fn summary(root: &Path, r: &Report, source: &str) -> Value {
    let cfg = store::config(root);
    let ps: Vec<&Problem> = r.problems.iter().filter(|p| p.source == source).collect();
    let errors = ps.iter().filter(|p| p.severity == "error").count();
    json!({
        "ok": errors == 0, "config": cfg, "errors": errors, "warnings": ps.len() - errors,
        "need_explanation": ps.iter().filter(|p| p.human.is_none()).count(),
        "runs": r.runs, "problems": ps.iter().take(25).collect::<Vec<_>>(), "more": ps.len().saturating_sub(25),
        "explain_with": "atlas write <file.json>  ·  [{\"problem\":\"<id>\",\"what\":\"…\",\"why\":\"…\",\"fix\":\"…\",\"try\":[\"<command>\"]}]",
    })
}

pub fn write_human(root: &Path, items: &[(&str, Human)]) -> (usize, Vec<String>) {
    let mut r = load(root);
    let mut n = 0;
    let mut unknown = Vec::new();
    for (id, h) in items {
        match r.problems.iter_mut().find(|p| p.id == *id) {
            Some(p) => {
                p.human = Some(Human { at: store::now(), ..h.clone() });
                n += 1;
            }
            None => unknown.push(id.to_string()),
        }
    }
    if n > 0 {
        let _ = save(root, &r);
    }
    (n, unknown)
}

// ---------------------------------------------------------------- human-readable views

pub fn problems_text(root: &Path, scopes: &[String]) -> String {
    let r = load(root);
    let th = store::config(root).lang == "th";
    let t = |a: &'static str, b: &'static str| if th { a } else { b };
    let ps: Vec<&Problem> = r.problems.iter().filter(|p| store::in_scope(&p.file, None, scopes)).collect();
    if ps.is_empty() {
        return t("ไม่พบปัญหา ✓  (ตรวจใหม่ได้ด้วย: atlas check)", "No problems found ✓  (re-check with: atlas check)").into();
    }
    let mut o = String::new();
    let errors = ps.iter().filter(|p| p.severity == "error").count();
    o.push_str(&format!("{} {} · {} {}\n", errors, t("ข้อผิดพลาด", "errors"), ps.len() - errors, t("คำเตือน", "warnings")));
    for (i, p) in ps.iter().enumerate() {
        let sev = if p.severity == "error" { t("ผิดพลาด", "ERROR") } else { t("เตือน", "WARNING") };
        o.push_str(&format!("\n━━ {}. [{sev}] {}:{}{}\n", i + 1, p.file, p.line, p.symbol.as_ref().map(|s| format!("  ({})", s.rsplit('#').next().unwrap_or(s))).unwrap_or_default()));
        match &p.human {
            Some(h) => {
                o.push_str(&format!("{} {}\n", t("เกิดอะไรขึ้น:", "What happened:"), h.what));
                if !h.why.is_empty() {
                    o.push_str(&format!("{} {}\n", t("ทำไม:", "Why:"), h.why));
                }
                if !h.fix.is_empty() {
                    o.push_str(&format!("{}\n{}\n", t("วิธีแก้:", "How to fix:"), h.fix));
                }
            }
            None => o.push_str(&format!("{} {}\n{}\n", t("ข้อความจากเครื่องมือ:", "Tool says:"), p.message, t("(ยังไม่มีคำอธิบายภาษาคน — สั่ง agent: /atlas check)", "(no plain-language explanation yet — ask: /atlas check)"))),
        }
        if !p.affects.is_empty() {
            o.push_str(&format!("{} {}\n", t("กระทบ:", "Affects:"), p.affects.join(", ")));
        }
        if !p.snippet.is_empty() {
            o.push_str(&format!("{}\n", p.snippet));
        }
        let mut cmds: Vec<String> = p.human.as_ref().map(|h| h.r#try.clone()).unwrap_or_default();
        for c in &p.rerun {
            if !cmds.contains(c) {
                cmds.push(c.clone());
            }
        }
        if !cmds.is_empty() {
            o.push_str(&format!("{}\n", t("ลองรันเอง:", "Try it yourself:")));
            for c in cmds {
                o.push_str(&format!("  $ {c}\n"));
            }
        }
    }
    o
}

pub fn trace_text(root: &Path, g: &Graph, key: &str, depth: usize) -> Result<String, String> {
    let s = g.symbols.iter().find(|s| s.id == key).or_else(|| {
        let m: Vec<_> = g.symbols.iter().filter(|s| s.name == key || s.name.ends_with(&format!(".{key}"))).collect();
        (m.len() == 1).then(|| m[0])
    });
    let Some(s) = s else { return Err(format!("not found: {key} (use a symbol id like src/a.ts#save, or a unique function name)")) };
    let by_id: HashMap<&str, &parse::Symbol> = g.symbols.iter().map(|s| (s.id.as_str(), s)).collect();
    let (called_by, _) = indexes(g);
    let th = store::config(root).lang == "th";
    let mut notes: BTreeMap<String, store::FileNotes> = BTreeMap::new();
    let mut sum = |x: &parse::Symbol| -> String {
        let n = notes.entry(x.file.clone()).or_insert_with(|| store::load_notes(root, &x.file));
        n.symbols.get(&x.id).map(|n| format!(" — {}", n.summary)).unwrap_or_default()
    };
    let mut o = format!("{}  ({}:{}){}\n", s.name, s.file, s.start, sum(s));
    let mut walk = |o: &mut String, up: bool| {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<(&str, usize)> = vec![(s.id.as_str(), 0)];
        let mut lines = Vec::new();
        while let Some((id, d)) = stack.pop() {
            if d >= depth {
                continue;
            }
            let next: Vec<&str> = if up { called_by.get(id).cloned().unwrap_or_default() } else { by_id.get(id).map(|x| x.calls.iter().map(|c| c.as_str()).collect()).unwrap_or_default() };
            for n in next.into_iter().rev() {
                if let Some(x) = by_id.get(n) {
                    let cyc = !seen.insert(n);
                    lines.push((d + 1, *x, cyc));
                    if !cyc {
                        stack.push((n, d + 1));
                    }
                }
            }
        }
        if lines.is_empty() {
            o.push_str("   —
");
        }
        for (d, x, cyc) in lines {
            let arrow = if up { "←" } else { "→" };
            o.push_str(&format!("{}{arrow} {}  ({}:{}){}{}\n", "   ".repeat(d), x.name, x.file, x.start, if cyc { " ↺" } else { "" }, sum(x)));
        }
    };
    o.push_str(if th { "\nถูกเรียกจาก (ถ้าส่วนนี้พัง ส่วนเหล่านี้จะได้รับผลกระทบ):\n" } else { "\nCalled by (affected if this breaks):\n" });
    walk(&mut o, true);
    o.push_str(if th { "\nเรียกใช้ (ถ้าส่วนเหล่านี้พัง ส่วนนี้จะพังตาม):\n" } else { "\nCalls (if these break, this breaks):\n" });
    walk(&mut o, false);
    Ok(o)
}
