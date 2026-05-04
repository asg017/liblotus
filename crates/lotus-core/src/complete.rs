//! Cursor-aware completion and signature help for the formula editor.
//!
//! Three entry points, all sheet-agnostic (the caller passes the in-scope
//! named ranges to `complete`):
//!
//! - [`list_functions`] — every builtin's metadata, for help panels.
//! - [`complete`] — identifier suggestions at a cursor position.
//! - [`signature_help`] — active function call + which parameter is being typed.
//!
//! Cursor analysis re-tokenizes `formula[0..cursor]` on each call. The lexer
//! is fast and formulas are short, so this is cheap. If the prefix fails to
//! lex (e.g. cursor sits inside an unterminated string), we return empty
//! results rather than guessing.

use crate::custom::Registry;
use crate::functions::{self, FunctionDef, FUNCTIONS};
use crate::lexer::Lexer;
use crate::types::{Token, TokenType};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionKind {
    Function,
    Name,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub kind: CompletionKind,
    /// Displayed in the menu.
    pub label: String,
    /// Text the editor should insert when the user accepts.
    pub insert: String,
    /// One-line summary (signature for functions, "Named range" for names).
    pub detail: Option<String>,
    /// Multi-line documentation (description + examples for functions).
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionList {
    /// Byte range in the formula string that the accepted insert replaces.
    /// Equal to `(cursor, cursor)` when there's no current prefix.
    pub replace_start: usize,
    pub replace_end: usize,
    pub items: Vec<CompletionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamInfo {
    pub name: String,
    pub optional: bool,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExampleInfo {
    pub formula: String,
    pub result: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionInfo {
    pub name: String,
    pub aliases: Vec<String>,
    pub category: String,
    pub params: Vec<ParamInfo>,
    pub variadic: Option<ParamInfo>,
    pub description: String,
    pub examples: Vec<ExampleInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureHelp {
    pub function: FunctionInfo,
    /// 0-based parameter index the cursor is in. Beyond `params.len()`
    /// when the call has entered the variadic tail.
    pub active_param: usize,
}

/// Metadata for every builtin function.
pub fn list_functions() -> Vec<FunctionInfo> {
    FUNCTIONS.iter().map(function_info).collect()
}

/// Like [`list_functions`] but also includes runtime-registered custom
/// functions from `registry`. Builtins come first in their static order,
/// followed by custom functions in registration order. Custom functions
/// produce a minimal [`FunctionInfo`] (name + open-ended variadic) — the
/// `CustomFunction` trait carries no docs/parameter metadata.
pub fn list_functions_with_registry(registry: &Registry) -> Vec<FunctionInfo> {
    let mut out: Vec<FunctionInfo> = FUNCTIONS.iter().map(function_info).collect();
    for f in registry.iter_functions() {
        out.push(custom_function_info(f.name()));
    }
    out
}

/// Completion suggestions for the cursor position. `names` are the
/// in-scope named ranges (typically `sheet.names()` keys, already uppercased).
///
/// Only built-in functions are suggested. Use [`complete_with_registry`]
/// to also surface runtime-registered custom functions (e.g. `lotus-datetime`
/// or `lotus-url` extensions).
pub fn complete(formula: &str, cursor: usize, names: &[&str]) -> CompletionList {
    complete_inner(formula, cursor, names, None)
}

/// Like [`complete`] but also suggests runtime-registered custom functions
/// from `registry`. Custom functions appear with a minimal `CustomFunction`
/// shape (name + open-ended variadic) — the trait carries no docs/parameter
/// metadata. Built-ins still appear first when names tie alphabetically
/// because they sort by label and no custom function shares a name with a
/// builtin (`Registry::register_function` rejects collisions).
pub fn complete_with_registry(
    registry: &Registry,
    formula: &str,
    cursor: usize,
    names: &[&str],
) -> CompletionList {
    complete_inner(formula, cursor, names, Some(registry))
}

fn complete_inner(
    formula: &str,
    cursor: usize,
    names: &[&str],
    registry: Option<&Registry>,
) -> CompletionList {
    // Only formulas get completions; raw values don't.
    if !formula.starts_with('=') {
        return empty_list(cursor);
    }
    let cursor = cursor.min(formula.len());
    let prefix_text = &formula[..cursor];

    // The lexer strips the leading `=` and reports positions relative to
    // the post-`=` input — compensate by adding 1 when reporting back.
    let tokens = match Lexer::new(prefix_text).tokenize() {
        Ok(t) => t,
        Err(_) => return empty_list(cursor), // e.g. inside an unterminated string
    };

    // Find the prefix being typed: the last identifier-shaped token whose
    // end coincides with the cursor.
    let (prefix, replace_start) = current_identifier_prefix(&tokens, prefix_text, cursor);

    // No identifier is being typed: only suggest when an identifier could
    // legally follow at this position (right after `=`, an operator, `(`,
    // `,`, etc.). After a value-producing token (number, string, closing
    // paren, completed cell ref) inserting a function name would just
    // produce a syntax error like `=200ABS(`.
    if prefix.is_empty() && !empty_prefix_allows_completion(&tokens) {
        return empty_list(cursor);
    }

    let prefix_upper = prefix.to_uppercase();
    let mut items = Vec::new();

    for def in FUNCTIONS {
        if def.name.starts_with(&prefix_upper) {
            items.push(function_item(def));
        }
    }
    if let Some(reg) = registry {
        for f in reg.iter_functions() {
            if f.name().starts_with(&prefix_upper) {
                items.push(custom_function_item(f.name()));
            }
        }
    }
    for name in names {
        let upper = name.to_uppercase();
        if upper.starts_with(&prefix_upper) {
            items.push(name_item(&upper));
        }
    }

    // Exact-prefix matches first, then alphabetical by label.
    items.sort_by(|a, b| a.label.cmp(&b.label));

    CompletionList {
        replace_start,
        replace_end: cursor,
        items,
    }
}

/// If the cursor is inside a function call's parens, return the function's
/// metadata and which parameter index is active. Otherwise `None`. Only
/// resolves built-in names; use [`signature_help_with_registry`] to also
/// resolve runtime-registered custom functions.
pub fn signature_help(formula: &str, cursor: usize) -> Option<SignatureHelp> {
    let (fn_name, active) = innermost_call_at_cursor(formula, cursor)?;
    let def = functions::lookup(&fn_name)?;
    Some(SignatureHelp {
        function: function_info(def),
        active_param: active,
    })
}

/// Like [`signature_help`] but falls back to `registry` when the function
/// name isn't a builtin. Returned `FunctionInfo` for a registered custom
/// function is minimal (name only); `active_param` still reflects the
/// cursor's comma depth so editors can highlight argument position.
pub fn signature_help_with_registry(
    registry: &Registry,
    formula: &str,
    cursor: usize,
) -> Option<SignatureHelp> {
    let (fn_name, active) = innermost_call_at_cursor(formula, cursor)?;
    if let Some(def) = functions::lookup(&fn_name) {
        return Some(SignatureHelp {
            function: function_info(def),
            active_param: active,
        });
    }
    let custom = registry.lookup_function(&fn_name)?;
    Some(SignatureHelp {
        function: custom_function_info(custom.name()),
        active_param: active,
    })
}

/// Walk `formula[..cursor]` and return the innermost in-progress function
/// call as `(name, active_param_index)`. Returns `None` if the cursor isn't
/// inside any real function-call parens.
fn innermost_call_at_cursor(formula: &str, cursor: usize) -> Option<(String, usize)> {
    if !formula.starts_with('=') {
        return None;
    }
    let cursor = cursor.min(formula.len());
    let prefix_text = &formula[..cursor];
    let tokens = Lexer::new(prefix_text).tokenize().ok()?;

    // Walk tokens forward, maintaining a stack of (function_name, comma_count_at_this_depth).
    // An entry is pushed when we see `Function LParen` and popped when we see the matching RParen.
    let mut stack: Vec<(String, usize)> = Vec::new();
    let mut i = 0;
    while i < tokens.len() && tokens[i].token_type != TokenType::Eof {
        let t = &tokens[i];
        match t.token_type {
            TokenType::Function => {
                // Only treat as a call if immediately followed by `(`.
                if matches!(tokens.get(i + 1), Some(n) if n.token_type == TokenType::LParen) {
                    stack.push((t.value.clone(), 0));
                    i += 2;
                    continue;
                }
            }
            TokenType::LParen => {
                // Bare `(` (parenthesized expression). Push a sentinel so
                // commas inside don't bump the enclosing call's param count.
                stack.push((String::new(), 0));
            }
            TokenType::RParen => {
                stack.pop();
            }
            TokenType::Comma => {
                if let Some((_, c)) = stack.last_mut() {
                    *c += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }

    // Find the innermost real function call (skip bare-paren sentinels).
    stack.into_iter().rev().find(|(n, _)| !n.is_empty())
}

// ── helpers ──────────────────────────────────────────────────────────

fn empty_list(cursor: usize) -> CompletionList {
    CompletionList {
        replace_start: cursor,
        replace_end: cursor,
        items: Vec::new(),
    }
}

/// Whether an identifier could legally follow at the cursor when no prefix
/// is being typed. Walks `tokens` (which covers `formula[..cursor]`) and
/// looks at the most-recent non-EOF token; returns true if that token is
/// one after which a name/function could appear.
fn empty_prefix_allows_completion(tokens: &[Token]) -> bool {
    for t in tokens.iter().rev() {
        if t.token_type == TokenType::Eof {
            continue;
        }
        return matches!(
            t.token_type,
            TokenType::Operator
                | TokenType::LParen
                | TokenType::LBrace
                | TokenType::Comma
                | TokenType::Semicolon
        );
    }
    // No prior tokens: cursor is right after `=` (possibly with whitespace).
    true
}

/// Last identifier-shaped token whose end aligns with the cursor, returned
/// as `(prefix_text, start_byte_in_full_formula)`. Returns `("", cursor)`
/// when the cursor isn't sitting on an identifier.
fn current_identifier_prefix(
    tokens: &[Token],
    prefix_text: &str,
    cursor: usize,
) -> (String, usize) {
    // Tokens hold positions relative to the post-`=` input; add 1 to map
    // back to byte offsets in the full formula.
    for t in tokens.iter().rev() {
        if t.token_type == TokenType::Eof {
            continue;
        }
        let token_end_in_full = t.position + t.source_len + 1;
        // Identifier token (lexer classifies bare letters as Function) or
        // CellRef in progress (typed letters but no digits yet would be a
        // Function — covered above; once digits appear it's a CellRef).
        if (t.token_type == TokenType::Function || t.token_type == TokenType::CellRef)
            && token_end_in_full == cursor
        {
            // Token positions are post-`=`; full-formula bytes need +1.
            let start_in_full = t.position + 1;
            let raw = &prefix_text[start_in_full..cursor];
            return (raw.to_string(), start_in_full);
        }
        // Stop scanning once we see anything else — nothing earlier can be
        // the immediately-preceding identifier.
        break;
    }
    (String::new(), cursor)
}

fn function_item(def: &FunctionDef) -> CompletionItem {
    CompletionItem {
        kind: CompletionKind::Function,
        label: def.name.to_string(),
        insert: format!("{}(", def.name),
        detail: Some(format_signature(def)),
        documentation: Some(format_documentation(def)),
    }
}

/// Completion entry for a runtime-registered custom function. Mirrors
/// `function_item` but with no detail/docs (the `CustomFunction` trait
/// carries no parameter or description metadata) and an `args, ...`
/// placeholder signature so editors render `NAME(args, ...)` instead of
/// `NAME()`.
fn custom_function_item(name: &str) -> CompletionItem {
    CompletionItem {
        kind: CompletionKind::Function,
        label: name.to_string(),
        insert: format!("{name}("),
        detail: Some(format!("{name}(args, ...)")),
        documentation: None,
    }
}

fn name_item(name: &str) -> CompletionItem {
    CompletionItem {
        kind: CompletionKind::Name,
        label: name.to_string(),
        insert: name.to_string(),
        detail: Some("Named range".to_string()),
        documentation: None,
    }
}

fn format_signature(def: &FunctionDef) -> String {
    let mut parts: Vec<String> = def
        .params
        .iter()
        .map(|p| {
            if p.optional {
                format!("{}?", p.name)
            } else {
                p.name.to_string()
            }
        })
        .collect();
    if let Some(v) = &def.variadic {
        parts.push(format!("{}, ...", v.name));
    }
    format!("{}({})", def.name, parts.join(", "))
}

fn format_documentation(def: &FunctionDef) -> String {
    let mut s = def.description.to_string();
    if !def.examples.is_empty() {
        s.push_str("\n\nExamples:");
        for ex in def.examples {
            s.push_str(&format!("\n  {} → {}", ex.formula, ex.result));
        }
    }
    s
}

/// Minimal `FunctionInfo` for a runtime-registered custom function. The
/// `CustomFunction` trait carries no docs/parameter metadata, so we
/// expose just the name plus an open-ended variadic so editors render
/// `NAME(args, ...)` rather than `NAME()`.
fn custom_function_info(name: &str) -> FunctionInfo {
    FunctionInfo {
        name: name.to_string(),
        aliases: Vec::new(),
        category: "Custom".to_string(),
        params: Vec::new(),
        variadic: Some(ParamInfo {
            name: "args".to_string(),
            optional: true,
            description: String::new(),
        }),
        description: String::new(),
        examples: Vec::new(),
    }
}

fn function_info(def: &FunctionDef) -> FunctionInfo {
    FunctionInfo {
        name: def.name.to_string(),
        aliases: def.aliases.iter().map(|s| s.to_string()).collect(),
        category: def.category.as_str().to_string(),
        params: def.params.iter().map(param_info).collect(),
        variadic: def.variadic.as_ref().map(param_info),
        description: def.description.to_string(),
        examples: def
            .examples
            .iter()
            .map(|e| ExampleInfo {
                formula: e.formula.to_string(),
                result: e.result.to_string(),
            })
            .collect(),
    }
}

fn param_info(p: &crate::functions::Param) -> ParamInfo {
    ParamInfo {
        name: p.name.to_string(),
        optional: p.optional,
        description: p.description.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(list: &CompletionList) -> Vec<&str> {
        list.items.iter().map(|i| i.label.as_str()).collect()
    }

    // ── list_functions ──

    #[test]
    fn list_functions_includes_every_builtin() {
        let names: Vec<String> = list_functions().into_iter().map(|f| f.name).collect();
        for expected in ["SUM", "AVERAGE", "MIN", "MAX", "COUNT", "ABS", "ROUND", "IF", "CONCAT", "LEN", "UPPER", "LOWER"] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[test]
    fn list_functions_attaches_aliases() {
        let avg = list_functions().into_iter().find(|f| f.name == "AVERAGE").unwrap();
        assert_eq!(avg.aliases, vec!["MEAN", "AVG"]);
    }

    #[test]
    fn list_functions_marks_variadic_and_required() {
        let sum = list_functions().into_iter().find(|f| f.name == "SUM").unwrap();
        assert!(sum.params.is_empty());
        assert!(sum.variadic.is_some());

        let round = list_functions().into_iter().find(|f| f.name == "ROUND").unwrap();
        assert_eq!(round.params.len(), 2);
        assert!(!round.params[0].optional);
        assert!(round.params[1].optional); // decimals defaults to 0
    }

    // ── complete ──

    #[test]
    fn no_completions_for_non_formula() {
        let list = complete("hello", 5, &[]);
        assert!(list.items.is_empty());
    }

    #[test]
    fn prefix_filters_function_list() {
        let list = complete("=SU", 3, &[]);
        let labels = labels(&list);
        assert!(labels.contains(&"SUM"));
        assert!(!labels.contains(&"AVERAGE"));
        // Replace the prefix range so accept overwrites "SU" (positions 1..3).
        assert_eq!(list.replace_start, 1);
        assert_eq!(list.replace_end, 3);
    }

    #[test]
    fn function_insert_includes_open_paren() {
        let list = complete("=SU", 3, &[]);
        let sum = list.items.iter().find(|i| i.label == "SUM").unwrap();
        assert_eq!(sum.insert, "SUM(");
    }

    #[test]
    fn empty_prefix_after_equals_lists_everything() {
        let list = complete("=", 1, &[]);
        // Should include all functions; no prefix → nothing to replace.
        assert!(list.items.len() >= 12);
        assert_eq!(list.replace_start, 1);
        assert_eq!(list.replace_end, 1);
    }

    #[test]
    fn names_appear_alongside_functions() {
        let list = complete("=R", 2, &["REVENUE", "RATE"]);
        let labels = labels(&list);
        assert!(labels.contains(&"REVENUE"));
        assert!(labels.contains(&"RATE"));
        assert!(labels.contains(&"ROUND"));
    }

    #[test]
    fn name_items_have_name_kind() {
        let list = complete("=REV", 4, &["REVENUE"]);
        let item = list.items.iter().find(|i| i.label == "REVENUE").unwrap();
        assert_eq!(item.kind, CompletionKind::Name);
        assert_eq!(item.insert, "REVENUE");
    }

    #[test]
    fn cursor_inside_unterminated_string_returns_empty() {
        // Lexer fails on `="hel` — be conservative.
        let list = complete("=\"hel", 5, &[]);
        assert!(list.items.is_empty());
    }

    #[test]
    fn no_prefix_inside_function_call() {
        // Cursor right after `(` — no identifier prefix, but completions still suggested.
        let list = complete("=SUM(", 5, &[]);
        let labels = labels(&list);
        assert!(labels.contains(&"SUM"));
        assert_eq!(list.replace_start, 5);
        assert_eq!(list.replace_end, 5);
    }

    #[test]
    fn case_insensitive_prefix_match() {
        let list = complete("=su", 3, &[]);
        let labels = labels(&list);
        assert!(labels.contains(&"SUM"));
    }

    #[test]
    fn no_completions_after_value_token_with_empty_prefix() {
        // `=200` — cursor at end. Empty identifier prefix, but the previous
        // token is a Number, so `=200ABS(` would be a syntax error. Don't
        // surface the entire function list here.
        assert!(complete("=200", 4, &[]).items.is_empty());
        // After a string literal, ditto.
        assert!(complete("=\"hi\"", 5, &[]).items.is_empty());
        // After a closing paren.
        assert!(complete("=SUM(1)", 7, &[]).items.is_empty());
        // After a completed cell ref + trailing space.
        assert!(complete("=A1 ", 4, &[]).items.is_empty());
    }

    #[test]
    fn completions_after_operator_or_open_paren() {
        // `=1+` — cursor after operator: identifier could follow.
        assert!(!complete("=1+", 3, &[]).items.is_empty());
        // `=SUM(` — cursor after `(`: identifier could follow.
        assert!(!complete("=SUM(", 5, &[]).items.is_empty());
        // `=SUM(1,` — cursor after `,`: identifier could follow.
        assert!(!complete("=SUM(1,", 7, &[]).items.is_empty());
    }

    // ── signature_help ──

    #[test]
    fn signature_help_inside_call_first_param() {
        let sig = signature_help("=SUM(", 5).unwrap();
        assert_eq!(sig.function.name, "SUM");
        assert_eq!(sig.active_param, 0);
    }

    #[test]
    fn signature_help_after_first_comma() {
        let sig = signature_help("=SUM(1, ", 8).unwrap();
        assert_eq!(sig.function.name, "SUM");
        assert_eq!(sig.active_param, 1);
    }

    #[test]
    fn signature_help_round_optional_param() {
        let sig = signature_help("=ROUND(3.14, ", 13).unwrap();
        assert_eq!(sig.function.name, "ROUND");
        assert_eq!(sig.active_param, 1);
        assert!(sig.function.params[1].optional);
    }

    #[test]
    fn signature_help_nested_call_picks_innermost() {
        // Cursor inside MAX(...) which is inside SUM(...).
        let sig = signature_help("=SUM(1, MAX(", 12).unwrap();
        assert_eq!(sig.function.name, "MAX");
        assert_eq!(sig.active_param, 0);
    }

    #[test]
    fn signature_help_alias_resolves_to_primary() {
        let sig = signature_help("=AVG(1, ", 8).unwrap();
        // AVG is an alias for AVERAGE; surface the canonical name.
        assert_eq!(sig.function.name, "AVERAGE");
    }

    #[test]
    fn signature_help_ignores_bare_parens() {
        // (1 + 2) is a parenthesized expression, not a call. Comma in this
        // context shouldn't bump anything — but here we're outside any call.
        let sig = signature_help("=(1 + 2", 7);
        assert!(sig.is_none());
    }

    #[test]
    fn signature_help_after_close_paren_returns_none() {
        let sig = signature_help("=SUM(1, 2)", 10);
        assert!(sig.is_none());
    }

    #[test]
    fn signature_help_unknown_function_returns_none() {
        let sig = signature_help("=FOOBAR(", 8);
        assert!(sig.is_none());
    }

    #[test]
    fn signature_help_for_non_formula_returns_none() {
        assert!(signature_help("hello", 3).is_none());
    }

    // ── registry-aware variants ──

    use crate::custom::CustomFunction;
    use crate::types::CellValue;
    use std::sync::Arc;

    struct DummyFn(&'static str);
    impl CustomFunction for DummyFn {
        fn name(&self) -> &str {
            self.0
        }
        fn call(&self, _args: &[CellValue]) -> Result<CellValue, String> {
            Ok(CellValue::Empty)
        }
    }

    fn registry_with(names: &[&'static str]) -> Registry {
        let mut r = Registry::new();
        for n in names {
            r.register_function(Arc::new(DummyFn(n))).unwrap();
        }
        r
    }

    #[test]
    fn list_functions_with_registry_appends_customs_after_builtins() {
        let baseline = list_functions().len();
        let r = registry_with(&["MY_FN", "OTHER_FN"]);
        let v = list_functions_with_registry(&r);
        assert_eq!(v.len(), baseline + 2);
        // Ordering: builtins first (preserved), then customs in registration order.
        assert_eq!(v[baseline].name, "MY_FN");
        assert_eq!(v[baseline + 1].name, "OTHER_FN");
    }

    #[test]
    fn custom_function_info_shape_is_minimal_variadic() {
        let r = registry_with(&["MY_FN"]);
        let v = list_functions_with_registry(&r);
        let info = v.iter().find(|f| f.name == "MY_FN").unwrap();
        assert!(info.aliases.is_empty());
        assert!(info.params.is_empty());
        assert!(info.variadic.is_some(), "should be open-ended variadic");
        assert_eq!(info.category, "Custom");
        assert!(info.examples.is_empty());
    }

    #[test]
    fn signature_help_with_registry_resolves_custom_function() {
        let r = registry_with(&["MY_FN"]);
        let sig = signature_help_with_registry(&r, "=MY_FN(", 7).unwrap();
        assert_eq!(sig.function.name, "MY_FN");
        assert_eq!(sig.active_param, 0);
    }

    #[test]
    fn signature_help_with_registry_advances_param_for_custom_function() {
        let r = registry_with(&["MY_FN"]);
        let sig = signature_help_with_registry(&r, "=MY_FN(1, 2, ", 13).unwrap();
        assert_eq!(sig.function.name, "MY_FN");
        assert_eq!(sig.active_param, 2);
    }

    #[test]
    fn signature_help_with_registry_prefers_builtin_over_custom_collision() {
        // Registry can never actually hold a name that collides with a
        // builtin (register_function rejects it), but the lookup order
        // matters if that ever changes — verify builtins win.
        let r = registry_with(&[]);
        let sig = signature_help_with_registry(&r, "=SUM(", 5).unwrap();
        assert_eq!(sig.function.name, "SUM");
        assert!(sig.function.variadic.is_some());
    }

    #[test]
    fn signature_help_with_registry_unknown_returns_none() {
        let r = registry_with(&["MY_FN"]);
        assert!(signature_help_with_registry(&r, "=NOT_REGISTERED(", 16).is_none());
    }

    #[test]
    fn complete_with_registry_surfaces_custom_function() {
        let r = registry_with(&["SPAN_TO_SECONDS", "SPAN_TO_DAYS"]);
        let list = complete_with_registry(&r, "=SPAN", 5, &[]);
        let labels = labels(&list);
        assert!(labels.contains(&"SPAN_TO_SECONDS"));
        assert!(labels.contains(&"SPAN_TO_DAYS"));
    }

    #[test]
    fn complete_with_registry_custom_function_insert_includes_open_paren() {
        let r = registry_with(&["SPAN_TO_SECONDS"]);
        let list = complete_with_registry(&r, "=SPAN_TO_S", 10, &[]);
        let item = list
            .items
            .iter()
            .find(|i| i.label == "SPAN_TO_SECONDS")
            .unwrap();
        assert_eq!(item.kind, CompletionKind::Function);
        assert_eq!(item.insert, "SPAN_TO_SECONDS(");
        // Custom functions get a placeholder `(args, ...)` signature so the
        // popup detail field isn't blank.
        assert_eq!(item.detail.as_deref(), Some("SPAN_TO_SECONDS(args, ...)"));
    }

    #[test]
    fn complete_with_registry_mixes_builtins_and_customs_alphabetically() {
        let r = registry_with(&["SPAN_TO_SECONDS"]);
        // Empty prefix after `=` lists everything; check both kinds appear.
        let list = complete_with_registry(&r, "=", 1, &[]);
        let labels = labels(&list);
        assert!(labels.contains(&"SUM"));
        assert!(labels.contains(&"SPAN_TO_SECONDS"));
        // Alphabetical: SPAN_TO_SECONDS < SUM.
        let span_idx = labels.iter().position(|l| *l == "SPAN_TO_SECONDS").unwrap();
        let sum_idx = labels.iter().position(|l| *l == "SUM").unwrap();
        assert!(span_idx < sum_idx, "expected alphabetical order");
    }

    #[test]
    fn complete_with_registry_case_insensitive_prefix() {
        let r = registry_with(&["SPAN_TO_SECONDS"]);
        let list = complete_with_registry(&r, "=span", 5, &[]);
        let labels = labels(&list);
        assert!(labels.contains(&"SPAN_TO_SECONDS"));
    }

    #[test]
    fn complete_with_registry_respects_value_token_gate() {
        // After a number, no identifier could legally follow — the gate
        // applies to custom functions just as it does to builtins.
        let r = registry_with(&["SPAN_TO_SECONDS"]);
        assert!(complete_with_registry(&r, "=200", 4, &[]).items.is_empty());
    }

    #[test]
    fn complete_with_registry_no_registry_matches_complete() {
        // Sanity: with an empty registry the result should equal complete().
        let r = Registry::new();
        let baseline = complete("=SU", 3, &[]);
        let with_reg = complete_with_registry(&r, "=SU", 3, &[]);
        assert_eq!(baseline, with_reg);
    }

    #[test]
    fn signature_help_with_registry_is_case_insensitive_for_custom() {
        let r = registry_with(&["MY_FN"]);
        let sig = signature_help_with_registry(&r, "=my_fn(", 7).unwrap();
        // Surfaces the registered (canonical) name, not the typed casing.
        assert_eq!(sig.function.name, "MY_FN");
    }
}
