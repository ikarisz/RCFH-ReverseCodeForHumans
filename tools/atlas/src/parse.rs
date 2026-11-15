//! tree-sitter parsing: symbols (functions, methods, classes, types), call
//! names inside each symbol, and a comment-free AST fingerprint used to prove
//! that inline explanations never change the code.

use serde::{Deserialize, Serialize};
use tree_sitter::{Language, Node, Parser, Tree};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Symbol {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub file: String,
    /// 1-based line where an explanation comment goes above (wrapper start: export / decorator / attributes)
    pub anchor: usize,
    pub start: usize,
    pub end: usize,
    pub sig: String,
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default)]
    pub calls: Vec<String>,
    #[serde(default, skip_serializing)]
    pub raw_calls: Vec<String>,
}

pub fn lang_of(path: &str) -> Option<&'static str> {
    let l = path.to_lowercase();
    if l.ends_with(".min.js") || l.ends_with(".d.ts") || l.ends_with(".bundle.js") {
        return None;
    }
    let ext = l.rsplit('.').next()?;
    Some(match ext {
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "py" | "pyw" => "python",
        "rs" => "rust",
        "go" => "go",
        _ => return None,
    })
}

pub fn language(lang: &str) -> Language {
    match lang {
        "javascript" => tree_sitter_javascript::LANGUAGE.into(),
        "typescript" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "python" => tree_sitter_python::LANGUAGE.into(),
        "rust" => tree_sitter_rust::LANGUAGE.into(),
        _ => tree_sitter_go::LANGUAGE.into(),
    }
}

pub fn parse(lang: &str, src: &str) -> Option<Tree> {
    let mut p = Parser::new();
    p.set_language(&language(lang)).ok()?;
    p.parse(src, None)
}

pub fn fnv(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

/// Hash of a node's code with whitespace and comments removed, so formatting
/// and explanation comments never mark an explanation as stale.
fn code_hash(n: Node, src: &str) -> String {
    let mut acc = String::new();
    fingerprint_into(n, src, &mut acc);
    fnv(&acc)
}

pub fn is_comment(kind: &str) -> bool {
    matches!(kind, "comment" | "line_comment" | "block_comment" | "doc_comment")
}

fn fingerprint_into(n: Node, src: &str, acc: &mut String) {
    if is_comment(n.kind()) {
        return;
    }
    acc.push_str(n.kind());
    acc.push('(');
    if n.child_count() == 0 {
        acc.push_str(n.utf8_text(src.as_bytes()).unwrap_or(""));
    } else {
        let mut c = n.walk();
        for ch in n.children(&mut c) {
            fingerprint_into(ch, src, acc);
        }
    }
    acc.push(')');
}

/// Comment-free AST fingerprint of the whole file (+ whether it has syntax errors).
pub fn fingerprint(lang: &str, src: &str) -> Option<(String, bool)> {
    let t = parse(lang, src)?;
    let mut acc = String::new();
    fingerprint_into(t.root_node(), src, &mut acc);
    Some((fnv(&acc), t.root_node().has_error()))
}

fn text<'a>(n: Node, src: &'a str) -> &'a str {
    n.utf8_text(src.as_bytes()).unwrap_or("")
}

fn first_line(n: Node, src: &str) -> String {
    let t = text(n, src);
    let l = t.lines().next().unwrap_or("").trim();
    let l = l.trim_end_matches('{').trim_end();
    l.chars().take(160).collect()
}

/// (kind label, name) if `n` defines a symbol.
fn symbol_def(n: Node, lang: &str, src: &str) -> Option<(&'static str, String)> {
    let name = |f: &str| n.child_by_field_name(f).map(|x| text(x, src).to_string());
    match lang {
        "javascript" | "typescript" | "tsx" => match n.kind() {
            "function_declaration" | "generator_function_declaration" => Some(("function", name("name")?)),
            "class_declaration" | "abstract_class_declaration" => Some(("class", name("name")?)),
            "method_definition" => Some(("method", name("name")?)),
            "interface_declaration" => Some(("interface", name("name")?)),
            "variable_declarator" => {
                let v = n.child_by_field_name("value")?;
                matches!(v.kind(), "arrow_function" | "function_expression" | "function" | "generator_function")
                    .then(|| ("function", name("name").unwrap_or_default()))
                    .filter(|(_, s)| !s.is_empty())
            }
            "public_field_definition" | "field_definition" => {
                let v = n.child_by_field_name("value")?;
                matches!(v.kind(), "arrow_function" | "function_expression")
                    .then(|| ("method", name("name").or_else(|| name("property")).unwrap_or_default()))
                    .filter(|(_, s)| !s.is_empty())
            }
            _ => None,
        },
        "python" => match n.kind() {
            "function_definition" => Some((if n.parent().and_then(|p| p.parent()).is_some_and(|g| g.kind() == "class_definition") || n.parent().and_then(|p| p.parent()).and_then(|p| p.parent()).is_some_and(|g| g.kind() == "class_definition") { "method" } else { "function" }, name("name")?)),
            "class_definition" => Some(("class", name("name")?)),
            _ => None,
        },
        "rust" => match n.kind() {
            "function_item" => Some((if n.parent().and_then(|p| p.parent()).is_some_and(|g| g.kind() == "impl_item" || g.kind() == "trait_item") { "method" } else { "function" }, name("name")?)),
            "struct_item" => Some(("struct", name("name")?)),
            "enum_item" => Some(("enum", name("name")?)),
            "trait_item" => Some(("trait", name("name")?)),
            "macro_definition" => Some(("macro", name("name")?)),
            _ => None,
        },
        _ => match n.kind() {
            "function_declaration" => Some(("function", name("name")?)),
            "method_declaration" => Some(("method", name("name")?)),
            "type_spec" => Some(("type", name("name")?)),
            _ => None,
        },
    }
}

/// Qualifier pushed by non-symbol containers (Rust impl blocks, Go receivers).
fn container_qual(n: Node, lang: &str, src: &str) -> Option<String> {
    if lang == "rust" && n.kind() == "impl_item" {
        let t = n.child_by_field_name("type")?;
        let s = text(t, src);
        return Some(s.split('<').next().unwrap_or(s).trim().to_string());
    }
    None
}

fn go_receiver(n: Node, src: &str) -> Option<String> {
    let r = n.child_by_field_name("receiver")?;
    let t = text(r, src);
    let t = t.trim_matches(|c| c == '(' || c == ')');
    let ty = t.split_whitespace().last()?;
    Some(ty.trim_start_matches('*').split('[').next().unwrap_or(ty).to_string())
}

/// Outermost wrapper whose start line should hold the explanation comment.
fn anchor_node<'a>(n: Node<'a>, lang: &str) -> Node<'a> {
    let mut a = n;
    match lang {
        "javascript" | "typescript" | "tsx" => {
            if n.kind() == "variable_declarator" {
                if let Some(p) = n.parent() {
                    if matches!(p.kind(), "lexical_declaration" | "variable_declaration") && p.named_child_count() == 1 {
                        a = p;
                    } else {
                        return n;
                    }
                }
            }
            while let Some(p) = a.parent() {
                if p.kind() == "export_statement" {
                    a = p;
                } else {
                    break;
                }
            }
        }
        "python" => {
            if let Some(p) = n.parent() {
                if p.kind() == "decorated_definition" {
                    a = p;
                }
            }
        }
        "rust" => {
            // attributes are preceding siblings
            let mut prev = a.prev_sibling();
            while let Some(p) = prev {
                if p.kind() == "attribute_item" {
                    a = p;
                    prev = p.prev_sibling();
                } else {
                    break;
                }
            }
        }
        _ => {}
    }
    a
}

/// Called name; member calls (`obj.save()`) are prefixed with '.' so they only
/// resolve to methods, never to an unrelated free function of the same name.
fn call_name(n: Node, lang: &str, src: &str) -> Option<String> {
    let js = matches!(lang, "javascript" | "typescript" | "tsx");
    // React/JSX handler references: onClick={handleSave}
    if js && n.kind() == "jsx_expression" {
        let c = n.named_child(0)?;
        return (n.named_child_count() == 1 && c.kind() == "identifier").then(|| text(c, src).to_string());
    }
    let f = match (lang, n.kind()) {
        ("python", "call") => n.child_by_field_name("function")?,
        (_, "call_expression") => n.child_by_field_name("function")?,
        (_, "new_expression") if js => n.child_by_field_name("constructor")?,
        _ => return None,
    };
    let (leaf, member) = match f.kind() {
        "identifier" => (f, false),
        "member_expression" => (f.child_by_field_name("property")?, true),
        "attribute" => (f.child_by_field_name("attribute")?, true),
        "field_expression" => (f.child_by_field_name("field")?, true),
        "scoped_identifier" => (f.child_by_field_name("name")?, false),
        "selector_expression" => (f.child_by_field_name("field")?, false),
        "generic_function" => (f.child_by_field_name("function")?, false),
        _ => return None,
    };
    let s = text(leaf, src).trim_start_matches('#');
    (!s.is_empty() && s.len() < 80).then(|| if member { format!(".{s}") } else { s.to_string() })
}

pub fn extract(file: &str, lang: &str, src: &str) -> Vec<Symbol> {
    let Some(tree) = parse(lang, src) else { return vec![] };
    let mut out: Vec<Symbol> = Vec::new();
    let mut quals: Vec<String> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    walk(tree.root_node(), file, lang, src, &mut quals, &mut stack, &mut out);
    // unique ids
    let mut seen = std::collections::HashMap::<String, usize>::new();
    for s in out.iter_mut() {
        let n = seen.entry(s.id.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            s.id = format!("{}~{}", s.id, n);
        }
    }
    out
}

fn walk(n: Node, file: &str, lang: &str, src: &str, quals: &mut Vec<String>, stack: &mut Vec<usize>, out: &mut Vec<Symbol>) {
    let mut pushed_sym = false;
    let mut pushed_qual = false;
    if let Some((kind, name)) = symbol_def(n, lang, src) {
        let mut q = quals.clone();
        if lang == "go" && n.kind() == "method_declaration" {
            if let Some(r) = go_receiver(n, src) {
                q.push(r);
            }
        }
        q.push(name.clone());
        let qual = q.join(".");
        let a = anchor_node(n, lang);
        let body = if n.kind() == "variable_declarator" || n.kind().ends_with("field_definition") { n } else { a };
        out.push(Symbol {
            id: format!("{file}#{qual}"),
            name: qual.clone(),
            kind: kind.into(),
            file: file.into(),
            anchor: a.start_position().row + 1,
            start: n.start_position().row + 1,
            end: n.end_position().row + 1,
            sig: first_line(if a.kind() == "attribute_item" { n } else { a }, src),
            hash: code_hash(body, src),
            parent: stack.last().map(|&i| out[i].id.clone()),
            calls: vec![],
            raw_calls: vec![],
        });
        stack.push(out.len() - 1);
        quals.push(name);
        pushed_sym = true;
        pushed_qual = true;
    } else if let Some(q) = container_qual(n, lang, src) {
        quals.push(q);
        pushed_qual = true;
    }
    if let Some(c) = call_name(n, lang, src) {
        if let Some(&i) = stack.last() {
            if !out[i].raw_calls.contains(&c) {
                out[i].raw_calls.push(c);
            }
        }
    }
    let mut cur = n.walk();
    for ch in n.children(&mut cur) {
        walk(ch, file, lang, src, quals, stack, out);
    }
    if pushed_sym {
        stack.pop();
    }
    if pushed_qual {
        quals.pop();
    }
}
