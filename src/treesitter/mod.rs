//! Tree-sitter AST integration.
//!
//! This module provides the foundational parse-tree infrastructure used by
//! text objects, code folding, and sticky scroll. `syntect` remains the source
//! of per-token colours; Tree-sitter adds the structural AST layer alongside it.

pub mod query;

use std::path::Path;
use tree_sitter::Parser;

// ── Language variant ─────────────────────────────────────────────────────────

/// Supported Tree-sitter languages.
///
/// Add new variants here when a new language grammar crate is added.
/// Each variant corresponds to exactly one tree-sitter grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TsLang {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    TypeScriptTsx,
    Go,
    Json,
    Bash,
}

impl TsLang {
    /// Detect the language from a file path using the file extension.
    ///
    /// Returns `None` for unknown or unsupported extensions.  Callers must
    /// handle `None` gracefully — Tree-sitter features simply degrade to
    /// unavailable for those files.
    pub fn detect(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        Some(match ext {
            "rs" => TsLang::Rust,
            "py" | "pyi" => TsLang::Python,
            "js" | "mjs" | "cjs" | "jsx" => TsLang::JavaScript,
            "ts" | "mts" | "cts" => TsLang::TypeScript,
            "tsx" => TsLang::TypeScriptTsx,
            "go" => TsLang::Go,
            "json" | "jsonc" => TsLang::Json,
            "sh" | "bash" | "zsh" | "fish" => TsLang::Bash,
            _ => return None,
        })
    }

    /// Human-readable label for diagnostics and UI.
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            TsLang::Rust => "Rust",
            TsLang::Python => "Python",
            TsLang::JavaScript => "JavaScript",
            TsLang::TypeScript => "TypeScript",
            TsLang::TypeScriptTsx => "TypeScript TSX",
            TsLang::Go => "Go",
            TsLang::Json => "JSON",
            TsLang::Bash => "Bash",
        }
    }

    fn ts_language(self) -> tree_sitter::Language {
        match self {
            TsLang::Rust => tree_sitter_rust::LANGUAGE.into(),
            TsLang::Python => tree_sitter_python::LANGUAGE.into(),
            TsLang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            TsLang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            TsLang::TypeScriptTsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            TsLang::Go => tree_sitter_go::LANGUAGE.into(),
            TsLang::Json => tree_sitter_json::LANGUAGE.into(),
            TsLang::Bash => tree_sitter_bash::LANGUAGE.into(),
        }
    }
}

// ── Parse snapshot ────────────────────────────────────────────────────────────

/// The result of a successful Tree-sitter parse.
///
/// Both `tree` and `source` are kept together so callers can resolve
/// byte-offset node ranges back to character positions.  The source is the
/// verbatim text that was passed to `TsEngine::parse()`.
pub struct TsSnapshot {
    /// The parse tree produced by Tree-sitter.
    pub tree: tree_sitter::Tree,
    /// The source text that was parsed (owned copy).
    /// Node byte ranges index into this string.
    pub source: String,
    /// Which grammar was used.
    pub lang: TsLang,
}

impl TsSnapshot {
    /// Return the UTF-8 text of a node, or an empty string if out of range.
    #[allow(dead_code)]
    pub fn node_text<'a>(&'a self, node: tree_sitter::Node<'_>) -> &'a str {
        self.source.get(node.byte_range()).unwrap_or("")
    }

    /// Convert a Tree-sitter (0-based row, 0-based column byte) point to a
    /// `(row, col)` pair suitable for use as a buffer cursor position.
    ///
    /// The column returned is the UTF-8 byte offset within the line, which
    /// matches how `Buffer` stores cursor columns.
    #[allow(dead_code)]
    pub fn point_to_rc(p: tree_sitter::Point) -> (usize, usize) {
        (p.row, p.column)
    }
}

// ── Engine ────────────────────────────────────────────────────────────────────

/// Wraps a `tree_sitter::Parser` and produces `TsSnapshot`s on demand.
///
/// A single `Parser` is reused across all calls (the language is reset before
/// each parse).  `Parser` is `!Send`, so `TsEngine` must live on the main
/// thread — the same constraint that already applies to `Highlighter`.
pub struct TsEngine {
    parser: Parser,
}

impl TsEngine {
    /// Create a new engine.  This is cheap — the `Parser` starts with no
    /// language assigned.
    pub fn new() -> Self {
        Self { parser: Parser::new() }
    }

    /// Detect the language for a file path.
    ///
    /// This is a convenience re-export of `TsLang::detect` so callers can
    /// use `TsEngine::detect(path)` without importing `TsLang`.
    pub fn detect(path: &Path) -> Option<TsLang> {
        TsLang::detect(path)
    }

    /// Parse `source` using the given language grammar.
    ///
    /// Returns `None` if `set_language` fails (grammar ABI mismatch) or if
    /// Tree-sitter times out / is cancelled (neither configured here, so the
    /// only failure path is ABI mismatch).
    pub fn parse(&mut self, source: &str, lang: TsLang) -> Option<TsSnapshot> {
        let ts_lang = lang.ts_language();
        self.parser.set_language(&ts_lang).ok()?;
        let tree = self.parser.parse(source, None)?;
        Some(TsSnapshot { tree, source: source.to_string(), lang })
    }
}

impl Default for TsEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detect_known_extensions() {
        let cases = [
            ("main.rs", Some(TsLang::Rust)),
            ("script.py", Some(TsLang::Python)),
            ("app.js", Some(TsLang::JavaScript)),
            ("app.ts", Some(TsLang::TypeScript)),
            ("app.tsx", Some(TsLang::TypeScriptTsx)),
            ("main.go", Some(TsLang::Go)),
            ("config.json", Some(TsLang::Json)),
            ("deploy.sh", Some(TsLang::Bash)),
            ("notes.txt", None),
            ("Makefile", None),
        ];
        for (name, expected) in cases {
            assert_eq!(TsLang::detect(&PathBuf::from(name)), expected, "file: {name}");
        }
    }

    #[test]
    fn parse_rust_source() {
        let mut engine = TsEngine::new();
        let src = "fn hello() -> u32 { 42 }";
        let snap = engine.parse(src, TsLang::Rust).expect("parse should succeed");
        let root = snap.tree.root_node();
        assert!(!root.has_error(), "expected error-free parse for valid Rust");
        assert_eq!(root.kind(), "source_file");
    }

    #[test]
    fn parse_json_source() {
        let mut engine = TsEngine::new();
        let src = r#"{"key": "value", "num": 42}"#;
        let snap = engine.parse(src, TsLang::Json).expect("parse should succeed");
        assert!(!snap.tree.root_node().has_error());
    }

    #[test]
    fn node_text_roundtrip() {
        let mut engine = TsEngine::new();
        let src = "fn hello() {}";
        let snap = engine.parse(src, TsLang::Rust).unwrap();
        let root = snap.tree.root_node();
        // The first named child of a source_file is typically the function item.
        if let Some(fn_node) = root.named_child(0) {
            let text = snap.node_text(fn_node);
            assert_eq!(text, src.trim());
        }
    }
}
