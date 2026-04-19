//! Tree-sitter AST node queries for text objects.
//!
//! All public functions take cursor positions in **char-index coordinates**
//! (matching `Buffer.cursor.col`) and return ranges in the same coordinates.
//! Byte↔char conversions are handled internally using the joined source string.

use tree_sitter::Node;

use super::{TsLang, TsSnapshot};
use crate::keymap::TextObjectKind;

// ── Coordinate helpers ────────────────────────────────────────────────────────

/// Convert a 0-based `(row, char_col)` buffer position to a byte offset in
/// `source` (the `"\n"`-joined buffer content).
///
/// `char_col` is a Unicode char index within the line, matching `Cursor.col`.
pub fn row_col_to_byte(source: &str, row: usize, char_col: usize) -> usize {
    let mut byte = 0usize;
    for (i, line) in source.split('\n').enumerate() {
        if i == row {
            let line_byte_offset =
                line.char_indices().nth(char_col).map(|(b, _)| b).unwrap_or(line.len());
            return byte + line_byte_offset;
        }
        byte += line.len() + 1; // +1 for the '\n' separator
    }
    source.len()
}

/// Count Unicode chars in `line` up to (but not including) `byte_col`.
///
/// Used to convert a tree-sitter byte-column back to a char index.
fn byte_col_to_char_idx(line: &str, byte_col: usize) -> usize {
    line.char_indices().take_while(|(i, _)| *i < byte_col).count()
}

/// Convert a tree-sitter **exclusive** end position `(row, byte_col)` to an
/// **inclusive** buffer cursor `(row, char_col)`.
///
/// Tree-sitter end positions point one position past the last byte of the node.
/// Buffer selections are inclusive, so we step back one character.
fn exclusive_byte_end_to_cursor(source: &str, row: usize, byte_col: usize) -> (usize, usize) {
    let lines: Vec<&str> = source.split('\n').collect();
    if byte_col > 0 {
        let line = lines.get(row).copied().unwrap_or("");
        // Count chars strictly before `byte_col` — that gives us the index of
        // the character whose byte range includes `byte_col - 1`.
        let char_count = line.char_indices().take_while(|(i, _)| *i < byte_col).count();
        (row, char_count.saturating_sub(1))
    } else if row > 0 {
        // byte_col == 0 means the end is at the start of `row`, so the last
        // included character is the last character of the previous row.
        let prev_line = lines.get(row - 1).copied().unwrap_or("");
        let last_char = prev_line.chars().count().saturating_sub(1);
        (row - 1, last_char)
    } else {
        (0, 0)
    }
}

// ── Node classification ───────────────────────────────────────────────────────

/// Returns true if `node` is a function or method definition in `lang`.
fn is_function_node(node: Node<'_>, lang: TsLang) -> bool {
    let k = node.kind();
    match lang {
        TsLang::Rust => k == "function_item",
        TsLang::Python => k == "function_definition",
        TsLang::JavaScript => {
            matches!(
                k,
                "function_declaration"
                    | "function_expression"
                    | "arrow_function"
                    | "method_definition"
            )
        },
        TsLang::TypeScript | TsLang::TypeScriptTsx => {
            matches!(
                k,
                "function_declaration"
                    | "function_expression"
                    | "arrow_function"
                    | "method_definition"
                    | "method_signature"
            )
        },
        TsLang::Go => matches!(k, "function_declaration" | "method_declaration"),
        TsLang::Json | TsLang::Bash => false,
    }
}

/// Returns true if `node` is a class / struct / impl / trait definition.
fn is_class_node(node: Node<'_>, lang: TsLang) -> bool {
    let k = node.kind();
    match lang {
        TsLang::Rust => matches!(k, "struct_item" | "enum_item" | "impl_item" | "trait_item"),
        TsLang::Python => k == "class_definition",
        TsLang::JavaScript => matches!(k, "class_declaration" | "class"),
        TsLang::TypeScript | TsLang::TypeScriptTsx => {
            matches!(k, "class_declaration" | "class" | "interface_declaration")
        },
        TsLang::Go => k == "type_declaration",
        TsLang::Json | TsLang::Bash => false,
    }
}

/// Returns true if `node` is a brace-delimited block.
fn is_block_node(node: Node<'_>, lang: TsLang) -> bool {
    let k = node.kind();
    match lang {
        TsLang::Rust | TsLang::Python | TsLang::Go => k == "block",
        TsLang::JavaScript | TsLang::TypeScript | TsLang::TypeScriptTsx => {
            matches!(k, "statement_block" | "class_body")
        },
        TsLang::Bash => k == "compound_statement",
        TsLang::Json => k == "object",
    }
}

// ── Ancestor walk ─────────────────────────────────────────────────────────────

/// Walk up the AST from the leaf at `(row, char_col)` to find the innermost
/// ancestor that satisfies `predicate`.
///
/// Returns `None` if no such ancestor exists (e.g., cursor is not inside any
/// function for `TextObjectKind::Function`).
pub fn ancestor_matching<'tree, F>(
    snap: &'tree TsSnapshot,
    row: usize,
    char_col: usize,
    predicate: F,
) -> Option<Node<'tree>>
where
    F: Fn(Node<'_>) -> bool,
{
    let byte = row_col_to_byte(&snap.source, row, char_col);
    let leaf = snap.tree.root_node().descendant_for_byte_range(byte, byte)?;
    let mut node = leaf;
    loop {
        if predicate(node) {
            return Some(node);
        }
        node = node.parent()?;
    }
}

// ── Body child ────────────────────────────────────────────────────────────────

/// Return the first "body" child of `parent` (the `{…}` block node).
///
/// Used for `inner` text object selections: `vif` selects the function body
/// block, while `vaf` selects the entire function node.
///
/// Returns `None` when no body child is found (e.g., an abstract method).
/// Callers fall back to `parent` in that case.
fn find_body_child<'tree>(parent: Node<'tree>, lang: TsLang) -> Option<Node<'tree>> {
    let body_kinds: &[&str] = match lang {
        TsLang::Rust | TsLang::Python | TsLang::Go => &["block"],
        TsLang::JavaScript | TsLang::TypeScript | TsLang::TypeScriptTsx => {
            &["statement_block", "class_body"]
        },
        TsLang::Bash => &["compound_statement"],
        TsLang::Json => return Some(parent), // JSON object IS its own body
    };
    (0..parent.child_count())
        .filter_map(|i| parent.child(i))
        .find(|n| body_kinds.contains(&n.kind()))
}

// ── Public query entry point ──────────────────────────────────────────────────

/// Compute the buffer range for a text object at the cursor position.
///
/// Returns `Some((start_row, start_char_col, end_row, end_char_col))` with all
/// coordinates in **inclusive** char-index notation matching `Buffer.cursor.col`.
///
/// Returns `None` when:
/// - No matching ancestor exists at the cursor.
/// - The language does not support the requested kind (e.g., Function in JSON).
///
/// `inner = true` selects the body block (e.g., the `{}` of a function).
/// `inner = false` selects the entire outer node including its signature.
pub fn text_object_range(
    snap: &TsSnapshot,
    row: usize,
    char_col: usize,
    inner: bool,
    kind: TextObjectKind,
) -> Option<(usize, usize, usize, usize)> {
    let outer = match kind {
        TextObjectKind::Function => {
            ancestor_matching(snap, row, char_col, |n| is_function_node(n, snap.lang))?
        },
        TextObjectKind::Class => {
            ancestor_matching(snap, row, char_col, |n| is_class_node(n, snap.lang))?
        },
        TextObjectKind::Block => {
            ancestor_matching(snap, row, char_col, |n| is_block_node(n, snap.lang))?
        },
    };

    let target = if inner { find_body_child(outer, snap.lang).unwrap_or(outer) } else { outer };

    let start = target.start_position();
    let end = target.end_position();

    let lines: Vec<&str> = snap.source.split('\n').collect();
    let start_char_col =
        lines.get(start.row).map(|l| byte_col_to_char_idx(l, start.column)).unwrap_or(0);
    let (end_row, end_char_col) = exclusive_byte_end_to_cursor(&snap.source, end.row, end.column);

    Some((start.row, start_char_col, end_row, end_char_col))
}

// ── Fold ranges ───────────────────────────────────────────────────────────────

/// Return all foldable regions in the parse tree, sorted by start row.
///
/// A region is foldable if it is a function/class/struct/impl node that spans
/// more than one line. Only named declaration-level nodes are included — inner
/// `{}` blocks are excluded to keep the fold list manageable.
pub fn fold_ranges(snap: &TsSnapshot) -> Vec<(usize, usize)> {
    let root = snap.tree.root_node();
    let mut ranges = Vec::new();
    collect_fold_ranges(root, snap.lang, &mut ranges);
    ranges.sort_by_key(|&(s, _)| s);
    ranges
}

fn collect_fold_ranges(node: Node<'_>, lang: TsLang, out: &mut Vec<(usize, usize)>) {
    let start = node.start_position().row;
    let end = node.end_position().row;
    if end > start && (is_function_node(node, lang) || is_class_node(node, lang)) {
        out.push((start, end));
    }
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_fold_ranges(child, lang, out);
        }
    }
}

// ── Sticky scroll ─────────────────────────────────────────────────────────────

/// Compute the sticky scroll context header for the given scroll position.
///
/// Returns the source text of the first line of the innermost enclosing
/// function or class that *started before* `scroll_row`, or `None` when the
/// viewport top is not inside any scope.
pub fn sticky_scroll_header(snap: &TsSnapshot, scroll_row: usize) -> Option<String> {
    if scroll_row == 0 {
        return None;
    }
    let node = ancestor_matching(snap, scroll_row, 0, |n| {
        (is_function_node(n, snap.lang) || is_class_node(n, snap.lang))
            && n.start_position().row < scroll_row
    })?;
    let header_row = node.start_position().row;
    snap.source.lines().nth(header_row).map(|l| l.to_string())
}

// ── Symbol extraction for agent tools ────────────────────────────────────────

/// A named definition extracted from a source file by the tree-sitter AST.
///
/// Used by `get_file_outline` and `get_symbol_context` in the agent tool layer
/// as a language-accurate replacement for the heuristic `extract_symbols`.
pub struct TsSymbolDef {
    /// 0-indexed line where the definition starts.
    pub line: usize,
    /// 0-indexed line where the definition ends.
    pub end_line: usize,
    /// Symbol name (identifier only, no keywords or types).
    pub name: String,
    /// First source line of the definition, trimmed.
    pub signature: String,
}

/// Extract all named top-level and class-member definitions from `source` using
/// the tree-sitter grammar for the file at `path`.
///
/// Returns `None` when the file extension is not supported (caller should fall
/// back to the heuristic extractor). The `Parser` is created and dropped within
/// this call — it is never held across an async boundary.
pub fn ts_extract_symbols(path: &std::path::Path, source: &str) -> Option<Vec<TsSymbolDef>> {
    let lang = TsLang::detect(path)?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang.ts_language()).ok()?;
    let tree = parser.parse(source.as_bytes(), None)?;
    let lines: Vec<&str> = source.lines().collect();
    let mut symbols = Vec::new();
    collect_ts_symbols(tree.root_node(), source, &lines, lang, false, &mut symbols);
    Some(symbols)
}

/// Recursive AST walk that emits `TsSymbolDef` entries.
///
/// `inside_class` controls depth: at the top level we look for functions,
/// classes, and arrow-function variable assignments; inside a class body we
/// look for methods only (we don't recurse further into method bodies).
fn collect_ts_symbols(
    node: Node<'_>,
    source: &str,
    lines: &[&str],
    lang: TsLang,
    inside_class: bool,
    out: &mut Vec<TsSymbolDef>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let kind = child.kind();

        // Transparent wrappers — pass through without emitting a symbol.
        // export_statement wraps function/class declarations in JS/TS.
        // decorated_definition wraps @decorator-annotated Python definitions.
        if matches!(
            kind,
            "export_statement" | "export_default_declaration" | "decorated_definition"
        ) {
            collect_ts_symbols(child, source, lines, lang, inside_class, out);
            continue;
        }

        // Variable declarations at the top level: look for arrow-function values.
        if !inside_class && matches!(kind, "lexical_declaration" | "variable_declaration") {
            collect_arrow_fn_vars(child, source, lines, out);
            continue;
        }

        let is_fn = is_function_node(child, lang);
        let is_cls = is_class_node(child, lang);

        if is_fn {
            // Emit function/method symbol; do not recurse into its body.
            if let Some(sym) = make_ts_symbol(child, source, lines) {
                out.push(sym);
            }
        } else if is_cls {
            // Emit class/struct/impl symbol; recurse into its body for members.
            if let Some(sym) = make_ts_symbol(child, source, lines) {
                out.push(sym);
            }
            collect_ts_symbols(child, source, lines, lang, true, out);
        } else if inside_class {
            // Inside a class: recurse only into body container nodes.
            if matches!(kind, "class_body" | "block" | "declaration_list" | "object_type") {
                collect_ts_symbols(child, source, lines, lang, true, out);
            }
        } else {
            // At the top level: recurse through non-definition, non-body nodes
            // (handles impl_item declaration_list, module wrappers, etc.).
            // Stop at function bodies so we don't descend into implementations.
            if !matches!(kind, "block" | "statement_block" | "function_body") {
                collect_ts_symbols(child, source, lines, lang, false, out);
            }
        }
    }
}

/// Emit a `TsSymbolDef` for a `variable_declarator` node whose value is an
/// arrow function or function expression (e.g. `const foo = (x) => x`).
fn collect_arrow_fn_vars(
    decl_node: Node<'_>,
    source: &str,
    lines: &[&str],
    out: &mut Vec<TsSymbolDef>,
) {
    let mut cursor = decl_node.walk();
    for child in decl_node.named_children(&mut cursor) {
        if child.kind() != "variable_declarator" {
            continue;
        }
        let value_is_fn = child
            .child_by_field_name("value")
            .map(|v| matches!(v.kind(), "arrow_function" | "function_expression"))
            .unwrap_or(false);
        if !value_is_fn {
            continue;
        }
        let Some(name_node) = child.child_by_field_name("name") else { continue };
        let name = source[name_node.byte_range()].to_string();
        if name.is_empty() {
            continue;
        }
        let line = decl_node.start_position().row;
        let end_line = decl_node.end_position().row;
        let signature = lines.get(line).unwrap_or(&"").trim().to_string();
        out.push(TsSymbolDef { line, end_line, name, signature });
    }
}

/// Build a `TsSymbolDef` from an AST node.
///
/// Returns `None` for anonymous nodes (e.g. bare arrow functions without a
/// surrounding variable declarator) so the caller can skip them cleanly.
fn make_ts_symbol(node: Node<'_>, source: &str, lines: &[&str]) -> Option<TsSymbolDef> {
    let name = ts_node_name(node, source)?.to_string();
    let line = node.start_position().row;
    let end_line = node.end_position().row;
    let signature = lines.get(line).unwrap_or(&"").trim().to_string();
    Some(TsSymbolDef { line, end_line, name, signature })
}

/// Extract the symbol name from an AST node.
///
/// Handles the special cases:
/// - Rust `impl_item` exposes a "type" field instead of "name".
/// - Anonymous `arrow_function` nodes have no name field at all.
/// - Fallback: first named identifier-kind child (covers some edge cases).
fn ts_node_name<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    // Rust impl blocks expose the implementing type via the "type" field.
    if node.kind() == "impl_item" {
        return node.child_by_field_name("type").map(|n| &source[n.byte_range()]);
    }
    // Anonymous arrow functions — name lives in the parent variable_declarator,
    // handled separately by collect_arrow_fn_vars.
    if node.kind() == "arrow_function" {
        return None;
    }
    // Standard "name" field used by function_item, struct_item, class_declaration,
    // function_declaration, method_definition, function_definition, etc.
    if let Some(n) = node.child_by_field_name("name") {
        let s = &source[n.byte_range()];
        if !s.is_empty() {
            return Some(s);
        }
    }
    // Fallback: first named child with an identifier-like kind.
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(
            child.kind(),
            "identifier" | "property_identifier" | "type_identifier" | "field_identifier"
        ) {
            let s = &source[child.byte_range()];
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::treesitter::TsEngine;

    fn parse_rust(src: &str) -> TsSnapshot {
        let mut engine = TsEngine::new();
        engine.parse(src, TsLang::Rust).expect("parse failed")
    }

    fn parse_python(src: &str) -> TsSnapshot {
        let mut engine = TsEngine::new();
        engine.parse(src, TsLang::Python).expect("parse failed")
    }

    #[test]
    fn outer_rust_function() {
        let src = "fn foo() {\n    let x = 1;\n}\n";
        let snap = parse_rust(src);
        // cursor inside body
        let r = text_object_range(&snap, 1, 4, false, TextObjectKind::Function);
        assert!(r.is_some(), "should find outer function");
        let (sr, sc, er, _ec) = r.unwrap();
        assert_eq!(sr, 0, "outer function starts at line 0");
        assert_eq!(sc, 0, "outer function starts at col 0");
        assert_eq!(er, 2, "outer function ends at line 2 (closing brace)");
    }

    #[test]
    fn inner_rust_function() {
        let src = "fn foo() {\n    let x = 1;\n}\n";
        let snap = parse_rust(src);
        let r = text_object_range(&snap, 1, 4, true, TextObjectKind::Function);
        assert!(r.is_some(), "should find inner function body");
        let (sr, _sc, _er, _ec) = r.unwrap();
        // The block starts on the same line as the opening brace (line 0)
        assert_eq!(sr, 0, "block starts on line 0 (the opening brace line)");
    }

    #[test]
    fn outer_rust_struct() {
        let src = "struct Foo {\n    x: u32,\n}\n";
        let snap = parse_rust(src);
        let r = text_object_range(&snap, 1, 4, false, TextObjectKind::Class);
        assert!(r.is_some(), "should find struct");
        let (sr, sc, _, _) = r.unwrap();
        assert_eq!((sr, sc), (0, 0));
    }

    #[test]
    fn no_function_at_top_level() {
        let src = "let x = 1;";
        let snap = parse_rust(src);
        let r = text_object_range(&snap, 0, 0, false, TextObjectKind::Function);
        assert!(r.is_none(), "no function at top level");
    }

    #[test]
    fn python_function() {
        let src = "def greet(name):\n    return name\n";
        let snap = parse_python(src);
        let r = text_object_range(&snap, 1, 4, false, TextObjectKind::Function);
        assert!(r.is_some());
        let (sr, sc, _, _) = r.unwrap();
        assert_eq!((sr, sc), (0, 0));
    }

    // ── ts_extract_symbols tests ─────────────────────────────────────────────

    fn extract(path: &str, src: &str) -> Vec<String> {
        let p = std::path::Path::new(path);
        ts_extract_symbols(p, src)
            .expect("ts_extract_symbols returned None")
            .into_iter()
            .map(|s| s.name)
            .collect()
    }

    #[test]
    fn ts_rust_top_level_fn() {
        let src = "fn foo() {}\nfn bar(x: u32) -> u32 { x }\n";
        let names = extract("mod.rs", src);
        assert!(names.contains(&"foo".to_string()));
        assert!(names.contains(&"bar".to_string()));
    }

    #[test]
    fn ts_rust_impl_methods() {
        let src = "struct Dog;\nimpl Dog {\n    fn bark(&self) {}\n    fn fetch(&self) {}\n}\n";
        let names = extract("lib.rs", src);
        assert!(names.contains(&"Dog".to_string()), "impl block itself: {names:?}");
        assert!(names.contains(&"bark".to_string()), "method bark: {names:?}");
        assert!(names.contains(&"fetch".to_string()), "method fetch: {names:?}");
    }

    #[test]
    fn ts_python_top_level_fn() {
        let src = "def greet(name):\n    return name\n\ndef farewell():\n    pass\n";
        let names = extract("utils.py", src);
        assert!(names.contains(&"greet".to_string()));
        assert!(names.contains(&"farewell".to_string()));
    }

    #[test]
    fn ts_python_class_methods() {
        let src = "class Dog:\n    def bark(self):\n        pass\n\n    def fetch(self, item):\n        return item\n";
        let names = extract("dog.py", src);
        assert!(names.contains(&"Dog".to_string()), "class: {names:?}");
        assert!(names.contains(&"bark".to_string()), "method bark: {names:?}");
        assert!(names.contains(&"fetch".to_string()), "method fetch: {names:?}");
    }

    #[test]
    fn ts_js_class_methods() {
        let src = "class Animal {\n    constructor(name) { this.name = name; }\n    speak() { console.log(this.name); }\n}\n";
        let names = extract("animal.js", src);
        assert!(names.contains(&"Animal".to_string()), "class: {names:?}");
        assert!(names.contains(&"constructor".to_string()), "constructor: {names:?}");
        assert!(names.contains(&"speak".to_string()), "speak: {names:?}");
    }

    #[test]
    fn ts_js_arrow_functions() {
        let src = "const add = (a, b) => a + b;\nconst mul = (a, b) => { return a * b; };\n";
        let names = extract("math.js", src);
        assert!(names.contains(&"add".to_string()), "add: {names:?}");
        assert!(names.contains(&"mul".to_string()), "mul: {names:?}");
    }

    #[test]
    fn ts_js_exported_class() {
        let src = "export class Service {\n    fetch(url) { return url; }\n}\n";
        let names = extract("service.js", src);
        assert!(names.contains(&"Service".to_string()), "class: {names:?}");
        assert!(names.contains(&"fetch".to_string()), "method: {names:?}");
    }

    #[test]
    fn ts_unsupported_ext_returns_none() {
        let p = std::path::Path::new("file.csharp");
        assert!(ts_extract_symbols(p, "class Foo {}").is_none());
    }

    #[test]
    fn ts_symbol_line_numbers() {
        let src = "fn first() {}\nfn second() {}\n";
        let p = std::path::Path::new("lib.rs");
        let syms = ts_extract_symbols(p, src).unwrap();
        assert_eq!(syms[0].line, 0);
        assert_eq!(syms[1].line, 1);
    }

    #[test]
    fn row_col_to_byte_ascii() {
        let source = "hello\nworld\n";
        assert_eq!(row_col_to_byte(source, 0, 0), 0);
        assert_eq!(row_col_to_byte(source, 0, 5), 5);
        assert_eq!(row_col_to_byte(source, 1, 0), 6);
        assert_eq!(row_col_to_byte(source, 1, 3), 9);
    }
}
