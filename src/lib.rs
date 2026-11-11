use intentumdiff_plugin_sdk::tree::{SemanticNode, SemanticNodeBuilder};

wit_bindgen::generate!({
    path: "wit/plugin.wit",
    world: "parser-plugin",
});

use crate::exports::intentdiff::plugin::parser::ExamplePair;
use crate::exports::intentdiff::plugin::parser::Guest;
use crate::exports::intentdiff::plugin::parser::LanguageInfoRecord;
use crate::exports::intentdiff::plugin::parser::ParserMode;

const LANGUAGE_ID: &str = "ocaml";
const ROOT_NODE_TYPE: &str = "ocaml_file";
const DETECT_EXTENSIONS: &[&str] = &[".ml", ".mli"];
const DEFAULT_OLD: &str = "let answer = 41\n";
const DEFAULT_NEW: &str = "let answer = 42\n";
const PLUGIN_METADATA: &str = include_str!("../plugin_metadata.info");

struct OcamlParser;

fn language_info_for(ids: Vec<String>) -> Vec<LanguageInfoRecord> {
    let metadata = intentumdiff_plugin_sdk::metadata::parse_plugin_metadata(PLUGIN_METADATA);
    ids.into_iter()
        .map(|language_id| {
            let info = metadata.language_or_default(&language_id);
            LanguageInfoRecord {
                language_id: info.language_id,
                language_name: info.language_name,
                language_short_name: info.language_short_name,
                monaco_language: info.monaco_language,
                default_filename: info.default_filename,
                language_file_extensions: info.language_file_extensions,
                author: metadata.author().to_string(),
                plugin_version: metadata.plugin_version().to_string(),
                last_updated: metadata.last_updated().to_string(),
            }
        })
        .collect()
}

fn detect_language_impl(filename: &str, _content: &str) -> String {
    let lower = filename.to_lowercase();
    if DETECT_EXTENSIONS.iter().any(|ext| lower.ends_with(ext)) {
        LANGUAGE_ID.to_string()
    } else {
        String::new()
    }
}

fn parse_ocaml(source: &str) -> String {
    let tokens = lex(source);
    let mut children = Vec::new();
    let total_lines = source.lines().count().saturating_sub(1) as u32;

    let mut start = 0;
    while start < tokens.len() {
        let line = tokens[start].line;
        let mut end = start + 1;
        while end < tokens.len() && tokens[end].line == line {
            end += 1;
        }
        let id = format!("0.{}", children.len());
        if let Some(child) = line_node(&id, &tokens[start..end]) {
            children.push(child);
        }
        start = end;
    }

    let root = SemanticNodeBuilder::new(
        "0",
        ROOT_NODE_TYPE,
        LANGUAGE_ID,
        0,
        0,
        total_lines,
        0,
        stable_hash(ROOT_NODE_TYPE, LANGUAGE_ID, &children),
    )
    .children(children)
    .build();

    match serde_json::to_string(&root) {
        Ok(serialized) => serialized,
        Err(err) => format!(r#"{{"error":"Serialisation error: {}"}}"#, err),
    }
}

/// One significant OCaml lexeme. Comments and layout never become tokens.
/// Positions are 0-based rows and UTF-8 byte columns into the original source.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Token {
    text: String,
    line: u32,
    col: u32,
    end_line: u32,
    end_col: u32,
}

impl Token {
    fn is_ident(&self) -> bool {
        self.text
            .chars()
            .next()
            .is_some_and(|ch| ch.is_alphabetic() || ch == '_')
    }
}

const OP_CHARS: &[u8] = b"!$%&*+-./:<=>?@^|~#";

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'\'' || b >= 0x80
}

/// Lexical scanner that tracks nested comments, string literals, quoted
/// strings and character literals, so comment markers inside literals stay
/// content (#7) and token-separating layout is never part of a label (#8).
/// Unterminated constructs run to end of input; it never panics.
fn lex(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut line_starts = vec![0usize];
    for (idx, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            line_starts.push(idx + 1);
        }
    }
    let position = |offset: usize| -> (u32, u32) {
        let line = line_starts.partition_point(|start| *start <= offset) - 1;
        (line as u32, (offset - line_starts[line]) as u32)
    };

    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        if b == b'(' && bytes.get(i + 1) == Some(&b'*') {
            i = skip_comment(bytes, i);
            continue;
        }
        i = if b == b'"' {
            skip_string(bytes, i)
        } else if let Some(end) = quoted_string_end(source, i) {
            end
        } else if b == b'\'' {
            char_literal_end(source, i).unwrap_or_else(|| {
                // Type variable such as 'a.
                let mut j = i + 1;
                while j < bytes.len() && is_ident_byte(bytes[j]) {
                    j += 1;
                }
                j
            })
        } else if b.is_ascii_digit() {
            number_end(bytes, i)
        } else if is_ident_byte(b) {
            ident_end(bytes, i)
        } else if (b == b'~' || b == b'?')
            && bytes
                .get(i + 1)
                .is_some_and(|n| n.is_ascii_lowercase() || *n == b'_')
        {
            // Labelled/optional argument `~x` / `?x:`, kept as one lexeme.
            let mut j = ident_end(bytes, i + 1);
            if bytes.get(j) == Some(&b':') && bytes.get(j + 1) != Some(&b':') {
                j += 1;
            }
            j
        } else if (b == b'[' && bytes.get(i + 1) == Some(&b'|'))
            || (b == b'|' && bytes.get(i + 1) == Some(&b']'))
        {
            i + 2
        } else if b == b';' {
            i + if bytes.get(i + 1) == Some(&b';') {
                2
            } else {
                1
            }
        } else if OP_CHARS.contains(&b) {
            let mut j = i;
            while j < bytes.len() && OP_CHARS.contains(&bytes[j]) {
                if bytes[j] == b'|' && bytes.get(j + 1) == Some(&b']') && j > i {
                    break;
                }
                j += 1;
            }
            j
        } else {
            // Brackets, commas and any other single character.
            i + source[i..].chars().next().map_or(1, char::len_utf8)
        };
        let i_end = i.min(bytes.len());
        let (line, col) = position(start);
        let (end_line, end_col) = position(i_end);
        tokens.push(Token {
            text: source[start..i_end].to_string(),
            line,
            col,
            end_line,
            end_col,
        });
        i = i_end;
    }
    tokens
}

fn skip_comment(bytes: &[u8], start: usize) -> usize {
    let mut depth = 0usize;
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'(' && bytes.get(i + 1) == Some(&b'*') {
            depth += 1;
            i += 2;
        } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b')') {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else if bytes[i] == b'"' {
            // OCaml lexes string literals inside comments.
            i = skip_string(bytes, i);
        } else {
            i += 1;
        }
    }
    bytes.len()
}

fn skip_string(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// `{|...|}` or `{id|...|id}` quoted string literal.
fn quoted_string_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes[start] != b'{' {
        return None;
    }
    let mut j = start + 1;
    while j < bytes.len() && (bytes[j].is_ascii_lowercase() || bytes[j] == b'_') {
        j += 1;
    }
    if bytes.get(j) != Some(&b'|') {
        return None;
    }
    let close = format!("|{}}}", &source[start + 1..j]);
    Some(
        source[j + 1..]
            .find(&close)
            .map_or(bytes.len(), |off| j + 1 + off + close.len()),
    )
}

fn char_literal_end(source: &str, start: usize) -> Option<usize> {
    let rest = &source[start + 1..];
    let mut chars = rest.char_indices();
    let (_, first) = chars.next()?;
    if first == '\\' {
        // Escapes are at most `\xFF`, `\255`, `\o377` or `\u{10FFFF}`.
        return rest
            .char_indices()
            .skip(2)
            .take(10)
            .find(|(_, ch)| *ch == '\'')
            .map(|(off, _)| start + 1 + off + 1);
    }
    let (off, second) = chars.next()?;
    (second == '\'' && first != '\'').then_some(start + 1 + off + 1)
}

fn number_end(bytes: &[u8], start: usize) -> usize {
    let hex = bytes[start] == b'0' && matches!(bytes.get(start + 1), Some(b'x' | b'X'));
    let mut i = start;
    while i < bytes.len() {
        let b = bytes[i];
        let exponent = i > start
            && if hex {
                matches!(bytes[i - 1], b'p' | b'P')
            } else {
                matches!(bytes[i - 1], b'e' | b'E')
            };
        if b.is_ascii_alphanumeric()
            || b == b'_'
            || b == b'.'
            || (exponent && (b == b'+' || b == b'-'))
        {
            i += 1;
        } else {
            break;
        }
    }
    i
}

fn ident_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() {
        if is_ident_byte(bytes[i]) {
            i += 1;
        } else if bytes[i] == b'.'
            && i > start
            && bytes
                .get(i + 1)
                .is_some_and(|n| n.is_ascii_alphabetic() || *n == b'_')
        {
            // Module path / field access: `List.map`, `r.field`.
            i += 1;
        } else {
            break;
        }
    }
    i
}

/// Canonical spelling of a token run: single spaces between lexemes, none
/// inside brackets or before separators. Two sources that differ only in
/// insignificant layout or comments get the same label and hash.
fn canonical(tokens: &[Token]) -> String {
    let mut out = String::new();
    for (idx, token) in tokens.iter().enumerate() {
        if idx > 0 {
            let prev = tokens[idx - 1].text.as_str();
            let cur = token.text.as_str();
            let tight_after = matches!(prev, "(" | "[" | "[|" | "{") && !cur.starts_with('*');
            let tight_before =
                matches!(cur, ")" | "]" | "|]" | "}" | "," | ";" | ";;") && !prev.ends_with('*');
            if !tight_after && !tight_before {
                out.push(' ');
            }
        }
        out.push_str(&token.text);
    }
    out
}

fn span_node(id: &str, node_type: &str, tokens: &[Token]) -> Option<SemanticNode> {
    let first = tokens.first()?;
    let last = tokens.last()?;
    let label = canonical(tokens);
    Some(
        SemanticNodeBuilder::new(
            id,
            node_type,
            &label,
            first.line,
            first.col,
            last.end_line,
            last.end_col,
            stable_hash(node_type, &label, &[]),
        )
        .build(),
    )
}

struct Declaration {
    node_type: &'static str,
    /// Index of the token naming the declaration.
    name: usize,
    /// Tokens between the keyword(s) and the name (`rec`, `!`, type params).
    modifiers_from: usize,
}

fn classify(tokens: &[Token]) -> Option<Declaration> {
    let text = |idx: usize| tokens.get(idx).map(|t| t.text.as_str());
    let ident_at = |idx: usize| tokens.get(idx).is_some_and(Token::is_ident);
    let decl = |node_type, name, modifiers_from| {
        ident_at(name).then_some(Declaration {
            node_type,
            name,
            modifiers_from,
        })
    };
    match text(0)? {
        "module" => {
            if text(1) == Some("type") {
                return decl("module_type", 2, 2);
            }
            let name = if text(1) == Some("rec") { 2 } else { 1 };
            let node_type = if text(name + 1) == Some(":") {
                "module_signature"
            } else {
                "module"
            };
            decl(node_type, name, 1)
        }
        "open" | "include" => {
            let name = if text(1) == Some("!") { 2 } else { 1 };
            let node_type = if text(0) == Some("open") {
                "open"
            } else {
                "include"
            };
            decl(node_type, name, 1)
        }
        "type" | "and" => {
            // The name is the last identifier before the definition operator;
            // anything before it (`nonrec`, `'a`, `('a, 'b)`) is a modifier.
            let head_end = tokens
                .iter()
                .position(|t| matches!(t.text.as_str(), "=" | ":=" | "+=" | ":"))
                .unwrap_or(tokens.len());
            let name = (1..head_end)
                .rev()
                .find(|idx| ident_at(*idx) && text(*idx) != Some("nonrec"))?;
            decl("type", name, 1)
        }
        "let" => {
            if text(1) == Some("rec") {
                decl("recursive_value", 2, 2)
            } else {
                decl("value", 1, 1)
            }
        }
        "external" => decl("external", 1, 1),
        "class" => {
            if text(1) == Some("type") {
                decl("class_type", 2, 2)
            } else {
                decl("class", 1, 1)
            }
        }
        "val" => decl("signature_value", 1, 1),
        _ => None,
    }
}

fn line_node(id: &str, tokens: &[Token]) -> Option<SemanticNode> {
    let Some(decl) = classify(tokens) else {
        // Unmodelled source is preserved conservatively as its full canonical
        // token sequence.
        return span_node(id, "ocaml_statement", tokens);
    };
    let mut children = Vec::new();
    if let Some(child) = span_node(
        &format!("{id}.mods"),
        "declaration_modifiers",
        &tokens[decl.modifiers_from..decl.name],
    ) {
        children.push(child);
    }
    let rest = &tokens[decl.name + 1..];
    if matches!(decl.node_type, "value" | "recursive_value") {
        // #46: the RHS is review content; parameters/annotations before `=`
        // are kept separately so neither is dropped.
        let eq = rest
            .iter()
            .position(|t| t.text == "=")
            .unwrap_or(rest.len());
        children.extend(span_node(&format!("{id}.head"), "value_head", &rest[..eq]));
        let body = rest.get(eq + 1..).unwrap_or(&[]);
        children.extend(span_node(&format!("{id}.rhs"), "value_body", body));
    } else {
        // #7: keep type definitions, signatures, module bodies/aliases and
        // class bodies instead of hashing the name only.
        let body = match rest.first().map(|t| t.text.as_str()) {
            Some("=" | ":") => &rest[1..],
            _ => rest,
        };
        children.extend(span_node(&format!("{id}.body"), "declaration_body", body));
    }
    let name = &tokens[decl.name];
    let label = name.text.as_str();
    SemanticNodeBuilder::new(
        id,
        decl.node_type,
        label,
        name.line,
        name.col,
        name.end_line,
        name.end_col,
        stable_hash(decl.node_type, label, &children),
    )
    .children(children)
    .build()
    .into()
}

fn stable_hash(node_type: &str, label: &str, children: &[SemanticNode]) -> String {
    let mut value = format!("{node_type}:{label}");
    for child in children {
        value.push('|');
        value.push_str(&child.structural_hash);
    }
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

impl Guest for OcamlParser {
    fn get_parser_mode() -> ParserMode {
        ParserMode::FullParse
    }

    fn grammar_id() -> String {
        LANGUAGE_ID.to_string()
    }

    fn detect_language(filename: String, content: String) -> String {
        detect_language_impl(&filename, &content)
    }

    fn preprocess_source(source: String) -> String {
        source
    }

    fn example(_language: String) -> ExamplePair {
        ExamplePair {
            old: DEFAULT_OLD.to_string(),
            new: DEFAULT_NEW.to_string(),
        }
    }

    fn process(input: String, _language: String, _filename: String) -> String {
        parse_ocaml(&input)
    }

    fn trivia_node_types() -> Vec<String> {
        vec![]
    }

    fn language_ids() -> Vec<String> {
        vec![LANGUAGE_ID.to_string()]
    }

    fn language_info() -> Vec<LanguageInfoRecord> {
        language_info_for(Self::language_ids())
    }

    fn priority() -> i32 {
        0
    }
}

export!(OcamlParser);

#[cfg(test)]
mod tests {
    #[test]
    fn value_body_uses_actual_source_columns() {
        let before = "let answer = 41\\n";
        let after = "let answer = 42\\n";

        let before_json: serde_json::Value =
            serde_json::from_str(&parse_ocaml(before)).expect("before JSON");
        let after_json: serde_json::Value =
            serde_json::from_str(&parse_ocaml(after)).expect("after JSON");

        let before_rhs = &before_json["children"][0]["children"][0];
        let after_rhs = &after_json["children"][0]["children"][0];

        assert_eq!(before_rhs["node_type"], "value_body");
        assert_eq!(before_rhs["position"]["start_col"], 13);
        assert_eq!(after_rhs["position"]["start_col"], 13);
        assert!(
            before_rhs["position"]["end_col"].as_u64().unwrap()
                > before_rhs["position"]["start_col"].as_u64().unwrap()
        );
    }

    use super::*;

    fn labels_by_type(node: &SemanticNode, node_type: &str, labels: &mut Vec<String>) {
        if node.node_type == node_type {
            labels.push(node.label.clone());
        }
        for child in &node.children {
            labels_by_type(child, node_type, labels);
        }
    }

    #[test]
    fn parser_mode_is_full_parse() {
        assert_eq!(OcamlParser::get_parser_mode(), ParserMode::FullParse);
    }

    #[test]
    fn grammar_id_is_language_id() {
        assert_eq!(OcamlParser::grammar_id(), LANGUAGE_ID);
        assert_eq!(OcamlParser::language_ids(), vec![LANGUAGE_ID.to_string()]);
    }

    #[test]
    fn detects_ocaml_extensions() {
        assert_eq!(detect_language_impl("main.ml", DEFAULT_NEW), LANGUAGE_ID);
        assert_eq!(detect_language_impl("main.mli", DEFAULT_NEW), LANGUAGE_ID);
    }

    #[test]
    fn process_returns_valid_json() {
        let parsed = parse_ocaml(DEFAULT_NEW);
        intentumdiff_plugin_sdk::testing::assert_valid_json(&parsed, LANGUAGE_ID);
        intentumdiff_plugin_sdk::testing::assert_root_node_type(
            &parsed,
            ROOT_NODE_TYPE,
            LANGUAGE_ID,
        );
    }

    #[test]
    fn process_extracts_modules_types_values_classes_and_signatures() {
        let parsed = parse_ocaml(
            r#"
open Core
include Shared
module Store : STORE = struct
type user = { id : string }
let rec load id = id
external digest : string -> string = "digest"
class cache = object end
class type readable = object end
val save : user -> unit
"#,
        );
        let root: SemanticNode = serde_json::from_str(&parsed).unwrap();
        let mut opens = Vec::new();
        let mut includes = Vec::new();
        let mut modules = Vec::new();
        let mut types = Vec::new();
        let mut values = Vec::new();
        let mut externals = Vec::new();
        let mut classes = Vec::new();
        let mut class_types = Vec::new();
        let mut signatures = Vec::new();
        labels_by_type(&root, "open", &mut opens);
        labels_by_type(&root, "include", &mut includes);
        labels_by_type(&root, "module_signature", &mut modules);
        labels_by_type(&root, "type", &mut types);
        labels_by_type(&root, "recursive_value", &mut values);
        labels_by_type(&root, "external", &mut externals);
        labels_by_type(&root, "class", &mut classes);
        labels_by_type(&root, "class_type", &mut class_types);
        labels_by_type(&root, "signature_value", &mut signatures);

        assert!(opens.contains(&"Core".to_string()));
        assert!(includes.contains(&"Shared".to_string()));
        assert!(modules.contains(&"Store".to_string()));
        assert!(types.contains(&"user".to_string()));
        assert!(values.contains(&"load".to_string()));
        assert!(externals.contains(&"digest".to_string()));
        assert!(classes.contains(&"cache".to_string()));
        assert!(class_types.contains(&"readable".to_string()));
        assert!(signatures.contains(&"save".to_string()));
    }

    #[test]
    fn value_labels_use_identifier_byte_columns_after_indentation() {
        // Previously the labelled value span covered "  let apply_dis..."
        // instead of the identifier. Its end column was truncated too.
        let source = "  let apply_discount = 42\n  let format_receipt = 3\n";
        let parsed: serde_json::Value = serde_json::from_str(&parse_ocaml(source)).unwrap();
        let bindings = parsed["children"].as_array().unwrap();
        assert_eq!(bindings.len(), 2);
        for (binding, label) in bindings.iter().zip(["apply_discount", "format_receipt"]) {
            assert_eq!(binding["label"], label);
            assert_eq!(binding["position"]["start_col"].as_u64(), Some(6));
            assert_eq!(
                binding["position"]["end_col"].as_u64(),
                Some(6 + label.len() as u64)
            );
        }
    }

    fn root_of(source: &str) -> SemanticNode {
        serde_json::from_str(&parse_ocaml(source)).expect("valid tree")
    }

    fn find<'a>(node: &'a SemanticNode, node_type: &str) -> Option<&'a SemanticNode> {
        if node.node_type == node_type {
            return Some(node);
        }
        node.children
            .iter()
            .find_map(|child| find(child, node_type))
    }

    fn assert_changed(old: &str, new: &str) {
        let (a, b) = (root_of(old), root_of(new));
        assert_ne!(
            a.structural_hash, b.structural_hash,
            "{old:?} -> {new:?} must change"
        );
    }

    fn assert_preserved(old: &str, new: &str) {
        let (a, b) = (root_of(old), root_of(new));
        assert_eq!(
            a.structural_hash, b.structural_hash,
            "{old:?} -> {new:?} must preserve"
        );
    }

    // #7: declaration payloads are part of the semantic projection.
    #[test]
    fn declaration_bodies_are_preserved() {
        assert_changed("type t = int\n", "type t = string\n");
        assert_changed("type answer = Yes\n", "type answer = Yes | No\n");
        assert_changed("val f : int -> int\n", "val f : int -> string\n");
        assert_changed("module M = A\n", "module M = B\n");
        assert_changed(
            "external f : int -> int = \"a\"\n",
            "external f : int -> int = \"b\"\n",
        );
        assert_changed("open M\n", "open! M\n");
        assert_changed("let f x = x\n", "let f y = x\n");
        assert_changed("type t =\n  | A\n", "type t =\n  | B\n");
        assert_changed("module type S = sig end\n", "module type T = sig end\n");
    }

    #[test]
    fn type_body_has_changed_content_and_source_span() {
        let root = root_of("type answer = Yes | No\n");
        let decl = find(&root, "type").expect("type node");
        assert_eq!(decl.label, "answer");
        assert_eq!((decl.position.start_col, decl.position.end_col), (5, 11));
        let body = find(decl, "declaration_body").expect("type body");
        assert_eq!(body.label, "Yes | No");
        assert_eq!((body.position.start_line, body.position.start_col), (0, 14));
        assert_eq!((body.position.end_line, body.position.end_col), (0, 22));

        let sig = root_of("val f : int -> string\n");
        assert_eq!(
            find(&sig, "declaration_body").unwrap().label,
            "int -> string"
        );
        let alias = root_of("module M = B\n");
        assert_eq!(find(&alias, "module").unwrap().label, "M");
        assert_eq!(find(&alias, "declaration_body").unwrap().label, "B");
    }

    #[test]
    fn type_parameters_do_not_become_the_name() {
        let root = root_of("type 'a t = 'a list\n");
        let decl = find(&root, "type").unwrap();
        assert_eq!(decl.label, "t");
        assert_eq!(find(decl, "declaration_modifiers").unwrap().label, "'a");
        assert_eq!(find(decl, "declaration_body").unwrap().label, "'a list");
    }

    // #7: comment markers inside literals are content, not comments.
    #[test]
    fn comment_markers_inside_strings_are_content() {
        assert_changed("let x = \"(*safe\"\n", "let x = \"(*danger\"\n");
        assert_changed("let x = {|(*safe|}\n", "let x = {|(*danger|}\n");
        assert_changed("let x = '(' (* c *)\n", "let x = ')' (* c *)\n");
        assert_changed("let x = \"a\\\"(*\" ^ b\n", "let x = \"a\\\"(*\" ^ c\n");
        let root = root_of("let x = \"(*safe\"\n");
        assert_eq!(find(&root, "value_body").unwrap().label, "\"(*safe\"");
    }

    #[test]
    fn comments_are_insignificant_including_nested_and_multiline() {
        assert_preserved("let x = 1\n", "let x = 1 (* note *)\n");
        assert_preserved(
            "let x = 1\n",
            "(* a\n (* nested \"*)\" *)\n b *)\nlet x = 1\n",
        );
        assert_preserved("let x = 1\n", "let x = (* inline *) 1\n");
        // An unterminated comment or string must not panic.
        root_of("let x = (* open\n");
        root_of("let x = \"open\n");
        root_of("let x = '\\\n");
    }

    // #8: token-separating spacing in a let RHS is preservation.
    #[test]
    fn let_rhs_spacing_is_preservation() {
        let old = "let x=1+2\nlet () = print_int x\n";
        let new = "let x = 1 + 2\nlet () = print_int x\n";
        assert_preserved(old, new);
        let root = root_of(new);
        let body = find(&root, "value_body").unwrap();
        assert_eq!(body.label, "1 + 2");
        assert_eq!((body.position.start_col, body.position.end_col), (8, 13));
        let compact = root_of(old);
        let body = find(&compact, "value_body").unwrap();
        assert_eq!(body.label, "1 + 2");
        assert_eq!((body.position.start_col, body.position.end_col), (6, 9));

        assert_preserved(
            "let () = print_int (answer ())\n",
            "let () = print_int ( answer () )\n",
        );
        assert_preserved("let a = [1;2]\n", "let a = [ 1; 2 ]\n");
        // Numeric and operator mutations stay meaningful.
        assert_changed(old, "let x=1+3\nlet () = print_int x\n");
        assert_changed(old, "let x=1-2\nlet () = print_int x\n");
        assert_changed("let x = a +- b\n", "let x = a + - b\n");
        assert_changed("let x = 1.5e-3\n", "let x = 1.5e - 3\n");
        assert_changed("let s = \"a b\"\n", "let s = \"a  b\"\n");
    }

    #[test]
    fn identity_is_byte_identical_and_deterministic() {
        let source = "let answer () = 1\nlet () = print_int (answer ())\n";
        assert_eq!(parse_ocaml(source), parse_ocaml(source));
        assert_preserved(source, source);
    }

    #[test]
    fn unicode_columns_are_utf8_bytes() {
        let root = root_of("let () = print_endline \"caf\u{e9}\" ^ x\n");
        let stmt = find(&root, "ocaml_statement").unwrap();
        assert_eq!(stmt.position.start_col, 0);
        // `"café"` is 7 bytes, then ` ^ x`.
        assert_eq!(stmt.position.end_col, 23 + 7 + 4);
        assert_changed(
            "let () = print_endline \"caf\u{e9}\"\n",
            "let () = print_endline \"caf\u{e8}\"\n",
        );
    }
}
