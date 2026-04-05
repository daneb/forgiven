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

    #[test]
    fn row_col_to_byte_ascii() {
        let source = "hello\nworld\n";
        assert_eq!(row_col_to_byte(source, 0, 0), 0);
        assert_eq!(row_col_to_byte(source, 0, 5), 5);
        assert_eq!(row_col_to_byte(source, 1, 0), 6);
        assert_eq!(row_col_to_byte(source, 1, 3), 9);
    }
}
