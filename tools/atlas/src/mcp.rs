//! Minimal MCP server over stdio (newline-delimited JSON-RPC). Same commands as the CLI.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::Path;

fn tools() -> Value {
    let scope = json!({"type": "array", "items": {"type": "string"}, "description": "files, folders (src/auth) or symbol ids (src/a.ts#save); empty = whole project"});
    json!([
        {"name": "atlas_status", "description": "Coverage of plain-language explanations: explained / outdated / missing, per folder.",
         "inputSchema": {"type": "object", "properties": {"scope": scope}}},
        {"name": "atlas_pending", "description": "Code that still needs a plain-language explanation (missing or outdated), with code, callers and callees. Only call when the user asked to explain code. Keep scope narrow to save tokens.",
         "inputSchema": {"type": "object", "properties": {"scope": scope, "changed": {"type": "boolean", "description": "only files changed in git"},
            "force": {"type": "boolean", "description": "re-explain even up-to-date code"}, "budget": {"type": "integer", "description": "max code lines (default 500)"},
            "limit": {"type": "integer", "description": "max symbols (default 25)"}}}},
        {"name": "atlas_write", "description": "Save explanations. items: [{id, summary, detail?}] for functions/classes, [{file, summary, detail?}] for files, [{dir, summary}] for folders (dir \"\" = whole project), [{problem, what, why, fix, try:[commands]}] for errors.",
         "inputSchema": {"type": "object", "required": ["items"], "properties": {"items": {"type": "array", "items": {"type": "object"}}}}},
        {"name": "atlas_show", "description": "Read stored explanations for a file, symbol id, or folder/.",
         "inputSchema": {"type": "object", "required": ["key"], "properties": {"key": {"type": "string"}}}},
        {"name": "atlas_check", "description": "Find errors in the project (syntax scan + installed checkers: tsc, python, ruff, cargo check, go vet; tests optional). Each problem comes with the function it is in, a code excerpt, who is affected and commands the user can re-run. Only when the user asks to check or debug.",
         "inputSchema": {"type": "object", "properties": {"scope": scope, "tests": {"type": "boolean", "description": "also run the test suite"}}}},
        {"name": "atlas_debug_log", "description": "Map pasted error output / a stack trace onto the code (functions, files, callers).",
         "inputSchema": {"type": "object", "required": ["output"], "properties": {"output": {"type": "string"}}}},
        {"name": "atlas_problems", "description": "Current problems in plain language (after explanations were written), with commands to try.",
         "inputSchema": {"type": "object", "properties": {"scope": scope}}},
        {"name": "atlas_trace", "description": "Who calls a function and what it calls (call chain), with plain-language summaries.",
         "inputSchema": {"type": "object", "required": ["key"], "properties": {"key": {"type": "string"}, "depth": {"type": "integer"}}}},
        {"name": "atlas_scan", "description": "Rebuild the code structure (files, functions, call graph). No AI involved.",
         "inputSchema": {"type": "object", "properties": {}}}
    ])
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.replace('\\', "/"))).collect()).unwrap_or_default()
}

fn call(root: &Path, name: &str, a: &Value) -> Result<String, String> {
    let v = match name {
        "atlas_status" => crate::status(root, &strs(&a["scope"]))?,
        "atlas_pending" => crate::pending(
            root,
            &crate::PendingOpts {
                scopes: strs(&a["scope"]),
                changed: a["changed"].as_bool().unwrap_or(false),
                force: a["force"].as_bool().unwrap_or(false),
                budget: a["budget"].as_u64().unwrap_or(500) as usize,
                limit: a["limit"].as_u64().unwrap_or(25) as usize,
                max_lines: 60,
            },
        )?,
        "atlas_write" => crate::write_items(root, a["items"].as_array().map(|v| v.as_slice()).unwrap_or(&[]))?,
        "atlas_show" => return crate::show(root, a["key"].as_str().unwrap_or("")),
        "atlas_check" => crate::check::check(root, &strs(&a["scope"]), a["tests"].as_bool().unwrap_or(false))?,
        "atlas_debug_log" => crate::check::debug(root, None, Some(a["output"].as_str().unwrap_or("").to_string()), false)?,
        "atlas_problems" => return Ok(crate::check::problems_text(root, &strs(&a["scope"]))),
        "atlas_trace" => {
            let g = crate::fresh_graph(root)?;
            return crate::check::trace_text(root, &g, a["key"].as_str().unwrap_or(""), a["depth"].as_u64().unwrap_or(3) as usize);
        }
        "atlas_scan" => {
            let g = crate::fresh_graph(root)?;
            json!({"ok": true, "files": g.files.len(), "symbols": g.symbols.len()})
        }
        _ => return Err(format!("unknown tool {name}")),
    };
    Ok(serde_json::to_string(&v).unwrap())
}

pub fn run(root: &Path) {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let Ok(req) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = req.get("id").cloned() else { continue }; // notification
        let method = req["method"].as_str().unwrap_or("");
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": req["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "codeatlas", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "CodeAtlas stores plain-language explanations of this codebase. Only explain code when the user asks: atlas_pending (narrow scope) → write explanations in the configured language/level → atlas_write."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => {
                let p = &req["params"];
                Ok(match call(root, p["name"].as_str().unwrap_or(""), &p["arguments"]) {
                    Ok(t) => json!({"content": [{"type": "text", "text": t}]}),
                    Err(e) => json!({"isError": true, "content": [{"type": "text", "text": e}]}),
                })
            }
            _ => Err(json!({"code": -32601, "message": format!("method not found: {method}")})),
        };
        let resp = match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
        };
        let _ = writeln!(stdout, "{resp}");
        let _ = stdout.flush();
    }
}
