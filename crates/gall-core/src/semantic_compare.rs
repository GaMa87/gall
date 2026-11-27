//! Comparison of two sources by their syntax trees (Rust, JavaScript, TypeScript), conservative by design.
//!
//! The answer is one of three: the files are **equivalent** (with the evidence), they **differ** (with the
//! changes, classified), or it is **unknown** (a language without a grammar here, a parse error, input over the
//! limits). A claim of equivalence is made only when the parse trees are equal after leaving out what the grammar
//! shows to be trivia:
//! * comments and zero-width tokens (the automatic semicolon of JavaScript);
//! * a trailing comma before a closing bracket, only in lists where it adds no element (not in Rust tuples, and
//!   not after an elision in a JavaScript array);
//! * the `;` that ends a JavaScript statement (when ASI changes how code is read, the *tree* differs and so the
//!   claim is not made);
//! * the order of Rust `use` declarations in a run (no evaluation order). The order of JavaScript imports is
//!   never ignored: modules are evaluated in that order.
//!
//! A tree that differs is classified, not excused: a consistent rename of local variables (proved with lexical
//! scope analysis across blocks and closures), a changed signature, a moved item, an added or removed item, or a
//! behavioural edit. Members inside `impl` blocks, traits, classes and modules are matched recursively when the
//! container header is equal. A rename is claimed only when it can be proved: names used inside macro arguments,
//! shorthand properties or struct fields, ambiguous `var` hoisting, and items with `eval` or `with` are never
//! renamed in the analysis, so they come out as behavioural edits.
//!
//! Raw byte ranges of the original text are kept for every change; nothing is rewritten.

use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::path::PathBuf;
use tree_sitter::{Node, Parser};

/// Largest source compared this way, in bytes.
pub const MAX_BYTES: usize = 4 * 1024 * 1024;
/// Deepest syntax tree followed; a deeper one is `unknown` (recursion must stay bounded).
const MAX_DEPTH: usize = 400;
/// Work between two looks at the cancellation flag.
const CANCEL_EVERY: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Language {
    Rust,
    JavaScript,
    TypeScript,
    /// TypeScript with JSX (`.tsx`): its own grammar, since `<T>` is a tag here and a cast there.
    Tsx,
}

/// The language of a file name, or `None` (then the comparison is `unknown`: unsupported).
pub fn language_of(name: &str) -> Option<Language> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some(Language::Rust),
        "js" | "mjs" | "cjs" | "jsx" => Some(Language::JavaScript),
        "ts" | "mts" | "cts" => Some(Language::TypeScript),
        "tsx" => Some(Language::Tsx),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Side {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "why", rename_all = "camelCase")]
pub enum Unknown {
    /// No grammar for this file type.
    Unsupported,
    /// The text does not parse cleanly (the first error is at `line`, 1-based).
    ParseError { side: Side, line: usize },
    /// Over the size or depth limits.
    TooLarge,
    /// The grammar could not be loaded.
    Grammar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Evidence {
    /// The same bytes.
    Identical,
    /// The same tree; only comments, layout, safe trailing delimiters or the order of Rust `use` lines differ.
    SameTree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    /// The names of local variables changed consistently; nothing else in the item.
    Rename,
    /// The parameters, the return type, the type parameters or the shape of a declaration changed.
    Signature,
    /// The same item stands elsewhere (same tree) in the other file.
    Moved,
    /// The body of an item changed.
    Behavior,
    Added,
    Removed,
}

/// A place in one source: the byte range and the 1-based line it starts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub kind: ChangeKind,
    pub name: Option<String>,
    pub left: Option<Span>,
    pub right: Option<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "verdict", rename_all = "camelCase")]
pub enum Verdict {
    Equivalent { evidence: Evidence },
    Different { changes: Vec<Change> },
    Unknown { reason: Unknown },
    Cancelled,
}

struct Ctx<'a> {
    lang: Language,
    src: &'a [u8],
    cancel: &'a dyn Fn() -> bool,
    work: usize,
    stopped: bool,
    too_deep: bool,
}

impl Ctx<'_> {
    fn tick(&mut self) {
        self.work += 1;
        if self.work.is_multiple_of(CANCEL_EVERY) && (self.cancel)() {
            self.stopped = true;
        }
    }
    fn text(&self, n: Node) -> &str {
        std::str::from_utf8(&self.src[n.start_byte()..n.end_byte()]).unwrap_or("")
    }
}

fn grammar(lang: Language) -> tree_sitter::Language {
    match lang {
        Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
    }
}

fn first_error_line(node: Node, depth: usize) -> usize {
    if node.is_error() || node.is_missing() || depth > MAX_DEPTH {
        return node.start_position().row + 1;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.has_error() {
            return first_error_line(child, depth + 1);
        }
    }
    node.start_position().row + 1
}

fn closer(kind: &str) -> bool {
    matches!(kind, ")" | "]" | "}")
}

/// Lists in which a comma before the closing bracket adds no element.
fn trailing_comma_is_trivia(lang: Language, parent: &str) -> bool {
    match lang {
        Language::Rust => matches!(
            parent,
            "arguments"
                | "array_expression"
                | "parameters"
                | "enum_variant_list"
                | "field_declaration_list"
                | "field_initializer_list"
                | "use_list"
                | "type_arguments"
                | "ordered_field_declaration_list"
        ),
        Language::JavaScript | Language::TypeScript | Language::Tsx => matches!(
            parent,
            "arguments"
                | "array"
                | "object"
                | "formal_parameters"
                | "named_imports"
                | "export_clause"
                | "type_arguments"
                | "enum_body"
                | "object_type"
        ),
    }
}

/// Statement kinds whose final `;` is a terminator (the tree shows whether ASI applied).
fn statement_terminator_is_trivia(lang: Language, parent: &str) -> bool {
    lang != Language::Rust
        && matches!(
            parent,
            "expression_statement"
                | "variable_declaration"
                | "lexical_declaration"
                | "return_statement"
                | "throw_statement"
                | "break_statement"
                | "continue_statement"
                | "do_statement"
                | "import_statement"
                | "export_statement"
                | "debugger_statement"
                | "type_alias_declaration"
                | "public_field_definition"
        )
}

/// Source text provided either directly in memory or as a path to read verbatim from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceInput {
    Text(String),
    File(PathBuf),
}

fn is_comment(node: Node) -> bool {
    node.is_extra()
        && matches!(
            node.kind(),
            "line_comment" | "block_comment" | "comment" | "html_comment"
        )
}

fn next_non_comment_sibling(mut node: Node) -> Option<Node> {
    while let Some(next) = node.next_sibling() {
        if !is_comment(next) {
            return Some(next);
        }
        node = next;
    }
    None
}

fn prev_non_comment_sibling(mut node: Node) -> Option<Node> {
    while let Some(prev) = node.prev_sibling() {
        if !is_comment(prev) {
            return Some(prev);
        }
        node = prev;
    }
    None
}

fn is_trivia_token(child: Node, parent: &str, lang: Language) -> bool {
    if child.child_count() != 0 || is_comment(child) {
        return false;
    }
    let kind = child.kind();
    if kind == ","
        && trailing_comma_is_trivia(lang, parent)
        && next_non_comment_sibling(child).is_some_and(|n| closer(n.kind()))
        && prev_non_comment_sibling(child)
            .is_some_and(|p| !matches!(p.kind(), "," | "[" | "(" | "{" | "<"))
    {
        return true;
    }
    kind == ";" && statement_terminator_is_trivia(lang, parent)
}

/// Writes the canonical form of a subtree: node kinds and leaf texts without trivia. `subst` renames local
/// identifiers to `$n`, and `skip_id` replaces one subtree (such as a container or function body) with a marker.
fn canon_inner(
    node: Node,
    cx: &mut Ctx,
    subst: &HashMap<String, usize>,
    skip_id: Option<usize>,
    depth: usize,
    out: &mut String,
) {
    cx.tick();
    if cx.stopped {
        return;
    }
    if depth > MAX_DEPTH {
        cx.too_deep = true;
        return;
    }
    if is_comment(node) {
        return;
    }
    if skip_id == Some(node.id()) {
        out.push_str("(\u{3}body\u{1f})");
        return;
    }
    if node.child_count() == 0 {
        let text = cx.text(node);
        // Zero-width unnamed tokens (such as JavaScript's automatic semicolon) carry no source text.
        if text.is_empty() && !node.is_named() {
            return;
        }
        if node.kind() == "identifier" {
            if let Some(n) = subst.get(text) {
                let _ = write!(out, "identifier:\u{2}{n}\u{1f}");
                return;
            }
        }
        // Length-prefixed, so that no text can imitate a separator.
        let _ = write!(out, "{}:{}:{text}\u{1f}", node.kind(), text.len());
        return;
    }
    let parent = node.kind();
    // In Rust, runs of consecutive unattributed `use` declarations inside a module body (`declaration_list`)
    // have no evaluation order, matching top-level `file_canon`.
    if cx.lang == Language::Rust
        && parent == "declaration_list"
        && subst.is_empty()
        && skip_id.is_none()
    {
        out.push('(');
        out.push_str(parent);
        out.push('\u{1f}');
        write_rust_use_sorted_children(node, cx, depth + 1, out);
        out.push(')');
        return;
    }
    out.push('(');
    out.push_str(parent);
    out.push('\u{1f}');
    let count = node.child_count();
    for i in 0..count {
        let Some(child) = node.child(i) else {
            continue;
        };
        if is_trivia_token(child, parent, cx.lang) {
            continue;
        }
        canon_inner(child, cx, subst, skip_id, depth + 1, out);
        if cx.stopped || cx.too_deep {
            return;
        }
    }
    out.push(')');
}

fn write_rust_use_sorted_children(node: Node, cx: &mut Ctx, depth: usize, out: &mut String) {
    let none = HashMap::new();
    let mut parts: Vec<(bool, String)> = Vec::new();
    let mut cursor = node.walk();
    let mut previous = "";
    for child in node.children(&mut cursor) {
        if is_comment(child) {
            continue;
        }
        // A `use` under an attribute (`#[cfg(unix)]`) keeps its place: the attribute belongs to that line.
        let is_use = cx.lang == Language::Rust
            && child.kind() == "use_declaration"
            && previous != "attribute_item";
        previous = child.kind();
        let mut s = String::new();
        canon_inner(child, cx, &none, None, depth, &mut s);
        if cx.stopped || cx.too_deep {
            return;
        }
        parts.push((is_use, s));
    }
    let mut i = 0;
    while i < parts.len() {
        if parts[i].0 {
            let mut j = i;
            while j < parts.len() && parts[j].0 {
                j += 1;
            }
            let mut run: Vec<&String> = parts[i..j].iter().map(|p| &p.1).collect();
            run.sort();
            for p in run {
                out.push_str(p);
            }
            i = j;
        } else {
            out.push_str(&parts[i].1);
            i += 1;
        }
    }
}

fn canon_of(node: Node, cx: &mut Ctx, subst: &HashMap<String, usize>) -> String {
    let mut out = String::new();
    canon_inner(node, cx, subst, None, 0, &mut out);
    out
}

fn header_canon(node: Node, body_id: usize, cx: &mut Ctx) -> String {
    let none = HashMap::new();
    let mut out = String::new();
    canon_inner(node, cx, &none, Some(body_id), 0, &mut out);
    out
}

/// The canonical form of a whole file; in Rust, runs of consecutive `use` declarations are put in order (the
/// order of imports has no meaning there).
fn file_canon(root: Node, cx: &mut Ctx) -> String {
    let mut out = String::from("(file\u{1f}");
    write_rust_use_sorted_children(root, cx, 0, &mut out);
    if cx.stopped || cx.too_deep {
        return String::new();
    }
    out.push(')');
    out
}

/// An item in a file or container body: a declaration with a name, or any other statement.
struct Item<'t> {
    key: String,
    name: Option<String>,
    node: Node<'t>,
    /// The declaration inside (`export` and decorators peeled off) and whether it is part of the public surface.
    decl: Node<'t>,
    exported: bool,
}

fn declared_name(cx: &Ctx, decl: Node) -> Option<String> {
    if decl.kind() == "impl_item" {
        let ty = decl
            .child_by_field_name("type")
            .map(|n| cx.text(n).to_string())?;
        return Some(match decl.child_by_field_name("trait") {
            Some(tr) => format!("{} for {ty}", cx.text(tr)),
            None => ty,
        });
    }
    if let Some(n) = decl.child_by_field_name("name") {
        let text = cx.text(n);
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    if matches!(decl.kind(), "lexical_declaration" | "variable_declaration") {
        let mut cursor = decl.walk();
        let first = decl
            .named_children(&mut cursor)
            .find(|c| c.kind() == "variable_declarator");
        if let Some(v) = first {
            if let Some(n) = v.child_by_field_name("name") {
                return Some(cx.text(n).to_string());
            }
        }
    }
    None
}

fn container_body<'t>(lang: Language, decl: Node<'t>) -> Option<Node<'t>> {
    let is_container = match lang {
        Language::Rust => matches!(decl.kind(), "impl_item" | "trait_item" | "mod_item"),
        Language::JavaScript | Language::TypeScript | Language::Tsx => matches!(
            decl.kind(),
            "class_declaration"
                | "abstract_class_declaration"
                | "class"
                | "internal_module"
                | "module"
        ),
    };
    if is_container {
        decl.child_by_field_name("body")
    } else {
        None
    }
}

fn method_qualifier(cx: &Ctx, decl: Node) -> String {
    let mut tags = Vec::new();
    let name_id = decl.child_by_field_name("name").map(|n| n.id());
    let mut cursor = decl.walk();
    for child in decl.children(&mut cursor) {
        if Some(child.id()) == name_id {
            break;
        }
        let text = cx.text(child);
        if matches!(text, "static" | "get" | "set") {
            tags.push(text);
        }
    }
    tags.join(":")
}

fn member_is_private(cx: &Ctx, decl: Node) -> bool {
    if let Some(name) = decl.child_by_field_name("name") {
        if name.kind() == "private_property_identifier" || cx.text(name).starts_with('#') {
            return true;
        }
    }
    let mut cursor = decl.walk();
    let found = decl
        .children(&mut cursor)
        .any(|c| c.kind() == "accessibility_modifier" && cx.text(c) == "private");
    found
}

fn member_separator(lang: Language) -> &'static str {
    if lang == Language::Rust {
        "::"
    } else {
        "."
    }
}

fn qualify_name(prefix: Option<&str>, name: Option<String>, lang: Language) -> Option<String> {
    match (prefix, name) {
        (Some(p), Some(n)) => Some(format!("{p}{}{n}", member_separator(lang))),
        (None, Some(n)) => Some(n),
        (Some(p), None) => Some(p.to_string()),
        (None, None) => None,
    }
}

fn items<'t>(root: Node<'t>, cx: &mut Ctx, inherited_export: bool) -> Vec<Item<'t>> {
    let mut out = Vec::new();
    let mut cursor = root.walk();
    for node in root.named_children(&mut cursor) {
        if is_comment(node) {
            continue;
        }
        let mut decl = node;
        let mut exported = inherited_export;
        if cx.lang != Language::Rust && node.kind() == "export_statement" {
            exported = true;
            if let Some(d) = node
                .child_by_field_name("declaration")
                .or_else(|| node.child_by_field_name("value"))
            {
                decl = d;
            }
        }
        if cx.lang == Language::Rust {
            let mut c = node.walk();
            if node
                .children(&mut c)
                .any(|k| k.kind() == "visibility_modifier")
            {
                exported = true;
            }
        } else if inherited_export && member_is_private(cx, decl) {
            exported = false;
        }
        let name = declared_name(cx, decl);
        let key = if decl.kind() == "impl_item" {
            if let Some(body) = decl.child_by_field_name("body") {
                format!("impl_item:{}", header_canon(node, body.id(), cx))
            } else {
                format!("#impl_item:{}", canon_of(node, cx, &HashMap::new()))
            }
        } else if decl.kind() == "method_definition" {
            match &name {
                Some(n) => {
                    let q = method_qualifier(cx, decl);
                    if q.is_empty() {
                        format!("method_definition:{n}")
                    } else {
                        format!("method_definition:{q}:{n}")
                    }
                }
                None => format!("#method_definition:{}", canon_of(node, cx, &HashMap::new())),
            }
        } else {
            match &name {
                Some(n) => format!("{}:{n}", decl.kind()),
                // An item without a name is told apart by its tree, so equal ones pair up wherever they stand.
                None => format!("#{}:{}", decl.kind(), canon_of(node, cx, &HashMap::new())),
            }
        };
        out.push(Item {
            key,
            name,
            node,
            decl,
            exported,
        });
    }
    out
}

fn span(node: Node) -> Span {
    Span {
        start: node.start_byte(),
        end: node.end_byte(),
        line: node.start_position().row + 1,
    }
}

/// Lexical scope state for proving local renames across nested blocks and closures.
struct ScopeState {
    scopes: Vec<HashMap<String, usize>>,
    /// Per enclosing function: number of nested block scopes above the function root, identifiers read,
    /// and `var` bindings declared (JavaScript/TypeScript hoisting guard).
    func_block_depths: Vec<usize>,
    func_uses: Vec<HashSet<String>>,
    func_vars: Vec<HashSet<String>>,
    next_id: usize,
    provable: bool,
}

impl ScopeState {
    fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            func_block_depths: vec![0],
            func_uses: vec![HashSet::new()],
            func_vars: vec![HashSet::new()],
            next_id: 0,
            provable: true,
        }
    }

    fn push_block(&mut self) {
        self.scopes.push(HashMap::new());
        if let Some(d) = self.func_block_depths.last_mut() {
            *d += 1;
        }
    }

    fn pop_block(&mut self) {
        self.scopes.pop();
        if let Some(d) = self.func_block_depths.last_mut() {
            *d = d.saturating_sub(1);
        }
    }

    fn push_func(&mut self) {
        self.scopes.push(HashMap::new());
        self.func_block_depths.push(0);
        self.func_uses.push(HashSet::new());
        self.func_vars.push(HashSet::new());
    }

    fn pop_func(&mut self) {
        self.scopes.pop();
        self.func_block_depths.pop();
        self.func_uses.pop();
        self.func_vars.pop();
    }

    fn alloc(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn bind_in_current(&mut self, name: String, id: usize) {
        if let Some(top) = self.scopes.last_mut() {
            top.insert(name, id);
        }
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        for scope in self.scopes.iter().rev() {
            if let Some(&id) = scope.get(name) {
                return Some(id);
            }
        }
        None
    }

    fn record_use(&mut self, name: &str) {
        if let Some(uses) = self.func_uses.last_mut() {
            uses.insert(name.to_string());
        }
    }

    fn bind_js_var(&mut self, name: String, id: usize) {
        let nested = self.func_block_depths.last().copied().unwrap_or(0) > 1;
        let used_before = self.func_uses.last().is_some_and(|u| u.contains(&name));
        let redeclared = self
            .func_vars
            .last_mut()
            .is_some_and(|v| !v.insert(name.clone()));
        if nested || used_before || redeclared {
            self.provable = false;
        }
        self.bind_in_current(name, id);
    }
}

fn contains_identifier_in(node: Node, cx: &Ctx, target: &HashSet<String>, depth: usize) -> bool {
    if depth > MAX_DEPTH || target.is_empty() {
        return false;
    }
    if node.kind() == "identifier" {
        return target.contains(cx.text(node));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if contains_identifier_in(child, cx, target, depth + 1) {
            return true;
        }
    }
    false
}

fn emit_leaf(node: Node, text: &str, out: &mut String) {
    if text.is_empty() && !node.is_named() {
        return;
    }
    let _ = write!(out, "{}:{}:{text}\u{1f}", node.kind(), text.len());
}

fn emit_bound(id: usize, out: &mut String) {
    let _ = write!(out, "identifier:\u{2}{id}\u{1f}");
}

/// Walks a binding pattern, writing its canonical form and collecting `(name, id)` into `pending`
/// without mutating the active scope yet.
fn alpha_pattern(
    node: Node,
    cx: &mut Ctx,
    st: &mut ScopeState,
    pending: &mut Vec<(String, usize)>,
    in_match_like: bool,
    depth: usize,
    out: &mut String,
) {
    cx.tick();
    if cx.stopped || !st.provable {
        return;
    }
    if depth > MAX_DEPTH {
        cx.too_deep = true;
        st.provable = false;
        return;
    }
    if is_comment(node) {
        return;
    }
    let kind = node.kind();
    if matches!(
        kind,
        "shorthand_property_identifier_pattern"
            | "shorthand_field_identifier"
            | "or_pattern"
            | "macro_invocation"
            | "token_tree"
    ) {
        st.provable = false;
        return;
    }
    if node.child_count() == 0 {
        let text = cx.text(node);
        if kind == "identifier" {
            // In Rust match / `if let` patterns, a bare uppercase identifier is likely a unit enum variant
            // or constant (`None`, `Ok`), never a local variable binding.
            let likely_variant = cx.lang == Language::Rust
                && in_match_like
                && text.chars().next().is_some_and(|c| c.is_ascii_uppercase());
            if !likely_variant && text != "_" {
                let id = st.alloc();
                pending.push((text.to_string(), id));
                emit_bound(id, out);
                return;
            }
        }
        emit_leaf(node, text, out);
        return;
    }
    out.push('(');
    out.push_str(kind);
    out.push('\u{1f}');
    let type_field_id = match kind {
        "tuple_struct_pattern" | "struct_pattern" => {
            node.child_by_field_name("type").map(|c| c.id())
        }
        _ => None,
    };
    let count = node.child_count();
    for i in 0..count {
        let Some(child) = node.child(i) else {
            continue;
        };
        if is_trivia_token(child, kind, cx.lang) {
            continue;
        }
        let cid = child.id();
        if kind == "assignment_pattern" {
            // `a = default`: only `left` binds; `right` is evaluated in the current scope.
            if node.child_by_field_name("left").map(|c| c.id()) == Some(cid) {
                alpha_pattern(child, cx, st, pending, in_match_like, depth + 1, out);
            } else {
                alpha_walk(child, cx, st, depth + 1, out);
            }
        } else if kind == "pair_pattern" {
            // `{ key: pat }` or `{ [expr]: pat }`: only `value` binds; `key` is a property name or computed expr.
            if node.child_by_field_name("value").map(|c| c.id()) == Some(cid) {
                alpha_pattern(child, cx, st, pending, in_match_like, depth + 1, out);
            } else {
                alpha_walk(child, cx, st, depth + 1, out);
            }
        } else if kind == "field_pattern" {
            // Rust `S { field: pat }`: only `pattern` binds.
            if node.child_by_field_name("pattern").map(|c| c.id()) == Some(cid) {
                alpha_pattern(child, cx, st, pending, in_match_like, depth + 1, out);
            } else {
                alpha_literal(child, cx, depth + 1, out);
            }
        } else if Some(cid) == type_field_id
            || matches!(
                child.kind(),
                "type_annotation" | "scoped_identifier" | "scoped_type_identifier"
            )
        {
            alpha_literal(child, cx, depth + 1, out);
        } else {
            alpha_pattern(child, cx, st, pending, in_match_like, depth + 1, out);
        }
        if cx.stopped || cx.too_deep || !st.provable {
            return;
        }
    }
    out.push(')');
}

/// Emits a subtree without substituting any identifier as a local variable (used for type positions,
/// qualified paths like `Foo::bar`, and item names).
fn alpha_literal(node: Node, cx: &mut Ctx, depth: usize, out: &mut String) {
    let none = HashMap::new();
    canon_inner(node, cx, &none, None, depth, out);
}

fn is_scope_function(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "closure_expression"
            | "function_declaration"
            | "function_expression"
            | "function"
            | "generator_function"
            | "generator_function_declaration"
            | "arrow_function"
            | "method_definition"
    )
}

fn is_jsx_intrinsic_name(node: Node, cx: &Ctx) -> bool {
    node.kind() == "identifier"
        && cx
            .text(node)
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase())
}

/// Walks an AST subtree, canonicalizing it with lexical De Bruijn-like binding IDs so consistent local
/// renames across nested scopes and closures produce identical canonical strings.
fn alpha_walk(node: Node, cx: &mut Ctx, st: &mut ScopeState, depth: usize, out: &mut String) {
    cx.tick();
    if cx.stopped || !st.provable {
        return;
    }
    if depth > MAX_DEPTH {
        cx.too_deep = true;
        st.provable = false;
        return;
    }
    if is_comment(node) {
        return;
    }
    let kind = node.kind();
    // Constructs that make renaming unprovable: runtime eval/with, macro arguments, and shorthand properties/fields.
    if matches!(
        kind,
        "with_statement"
            | "macro_invocation"
            | "macro_definition"
            | "shorthand_property_identifier"
            | "shorthand_property_identifier_pattern"
            | "shorthand_field_initializer"
            | "shorthand_field_identifier"
            | "token_tree"
    ) {
        st.provable = false;
        return;
    }
    if kind == "identifier" && cx.text(node) == "eval" {
        st.provable = false;
        return;
    }
    if matches!(
        kind,
        "scoped_identifier"
            | "scoped_type_identifier"
            | "type_annotation"
            | "type_parameters"
            | "type_arguments"
    ) {
        alpha_literal(node, cx, depth, out);
        return;
    }
    if node.child_count() == 0 {
        let text = cx.text(node);
        if kind == "identifier" {
            if let Some(id) = st.lookup(text) {
                emit_bound(id, out);
            } else {
                st.record_use(text);
                emit_leaf(node, text, out);
            }
        } else {
            emit_leaf(node, text, out);
        }
        return;
    }

    // Specialized scoped constructs.
    match kind {
        "block" | "statement_block" | "switch_body" => {
            st.push_block();
            alpha_walk_children(node, cx, st, depth, out);
            st.pop_block();
            return;
        }
        "let_declaration" => {
            // Rust `let pat: T = value else { alt };` — `value` and `alt` are evaluated before `pat` is bound.
            out.push('(');
            out.push_str(kind);
            out.push('\u{1f}');
            let pat_id = node.child_by_field_name("pattern").map(|c| c.id());
            let mut pending = Vec::new();
            for i in 0..node.child_count() {
                let Some(child) = node.child(i) else { continue };
                if is_trivia_token(child, kind, cx.lang) {
                    continue;
                }
                if Some(child.id()) == pat_id {
                    alpha_pattern(child, cx, st, &mut pending, false, depth + 1, out);
                } else {
                    alpha_walk(child, cx, st, depth + 1, out);
                }
                if cx.stopped || cx.too_deep || !st.provable {
                    return;
                }
            }
            out.push(')');
            for (name, id) in pending {
                st.bind_in_current(name, id);
            }
            return;
        }
        "lexical_declaration" | "variable_declaration" => {
            let is_var = kind == "variable_declaration";
            out.push('(');
            out.push_str(kind);
            out.push('\u{1f}');
            for i in 0..node.child_count() {
                let Some(child) = node.child(i) else { continue };
                if is_trivia_token(child, kind, cx.lang) {
                    continue;
                }
                if child.kind() == "variable_declarator" {
                    alpha_var_declarator(child, cx, st, is_var, depth + 1, out);
                } else {
                    alpha_walk(child, cx, st, depth + 1, out);
                }
                if cx.stopped || cx.too_deep || !st.provable {
                    return;
                }
            }
            out.push(')');
            return;
        }
        "for_expression" => {
            // Rust `for pat in value { body }`: `value` runs in outer scope, `pat` binds inside a new scope for `body`.
            out.push('(');
            out.push_str(kind);
            out.push('\u{1f}');
            let pat_id = node.child_by_field_name("pattern").map(|c| c.id());
            let body_id = node.child_by_field_name("body").map(|c| c.id());
            let mut pending = Vec::new();
            for i in 0..node.child_count() {
                let Some(child) = node.child(i) else { continue };
                if is_trivia_token(child, kind, cx.lang) {
                    continue;
                }
                let cid = child.id();
                if Some(cid) == pat_id {
                    alpha_pattern(child, cx, st, &mut pending, false, depth + 1, out);
                } else if Some(cid) == body_id {
                    st.push_block();
                    for (name, id) in pending.drain(..) {
                        st.bind_in_current(name, id);
                    }
                    alpha_walk(child, cx, st, depth + 1, out);
                    st.pop_block();
                } else {
                    alpha_walk(child, cx, st, depth + 1, out);
                }
                if cx.stopped || cx.too_deep || !st.provable {
                    return;
                }
            }
            out.push(')');
            return;
        }
        "for_statement" => {
            st.push_block();
            alpha_walk_children(node, cx, st, depth, out);
            st.pop_block();
            return;
        }
        "for_in_statement" => {
            // JS/TS `for (const x of xs) body`: `right` sees outer scope (and TDZ check); `left` binds in loop scope.
            out.push('(');
            out.push_str(kind);
            out.push('\u{1f}');
            let left_id = node.child_by_field_name("left").map(|c| c.id());
            let right_id = node.child_by_field_name("right").map(|c| c.id());
            let body_id = node.child_by_field_name("body").map(|c| c.id());
            let is_var = node
                .child_by_field_name("kind")
                .is_some_and(|k| cx.text(k) == "var")
                || (0..node.child_count())
                    .filter_map(|i| node.child(i))
                    .any(|c| cx.text(c) == "var");
            let is_lexical = !is_var
                && (0..node.child_count())
                    .filter_map(|i| node.child(i))
                    .any(|c| matches!(cx.text(c), "let" | "const"));
            let mut pending = Vec::new();
            for i in 0..node.child_count() {
                let Some(child) = node.child(i) else { continue };
                if is_trivia_token(child, kind, cx.lang) {
                    continue;
                }
                let cid = child.id();
                if Some(cid) == left_id && (is_lexical || is_var) {
                    alpha_pattern(child, cx, st, &mut pending, false, depth + 1, out);
                } else if Some(cid) == right_id {
                    if is_lexical {
                        let names: HashSet<String> =
                            pending.iter().map(|(n, _)| n.clone()).collect();
                        if contains_identifier_in(child, cx, &names, 0) {
                            st.provable = false;
                            return;
                        }
                    }
                    alpha_walk(child, cx, st, depth + 1, out);
                } else if Some(cid) == body_id {
                    if is_lexical {
                        st.push_block();
                        for (name, id) in pending.drain(..) {
                            st.bind_in_current(name, id);
                        }
                        alpha_walk(child, cx, st, depth + 1, out);
                        st.pop_block();
                    } else {
                        for (name, id) in pending.drain(..) {
                            st.bind_js_var(name, id);
                        }
                        alpha_walk(child, cx, st, depth + 1, out);
                    }
                } else {
                    alpha_walk(child, cx, st, depth + 1, out);
                }
                if cx.stopped || cx.too_deep || !st.provable {
                    return;
                }
            }
            out.push(')');
            return;
        }
        "if_expression" | "while_expression" => {
            // Rust `if let pat = val { cons } else { alt }` and `while let pat = val { body }`.
            out.push('(');
            out.push_str(kind);
            out.push('\u{1f}');
            let cond_id = node.child_by_field_name("condition").map(|c| c.id());
            let cons_id = node
                .child_by_field_name("consequence")
                .or_else(|| node.child_by_field_name("body"))
                .map(|c| c.id());
            let mut pending = Vec::new();
            for i in 0..node.child_count() {
                let Some(child) = node.child(i) else { continue };
                if is_trivia_token(child, kind, cx.lang) {
                    continue;
                }
                let cid = child.id();
                if Some(cid) == cond_id && child.kind() == "let_condition" {
                    alpha_let_condition(child, cx, st, &mut pending, depth + 1, out);
                } else if Some(cid) == cons_id && !pending.is_empty() {
                    st.push_block();
                    for (name, id) in pending.drain(..) {
                        st.bind_in_current(name, id);
                    }
                    alpha_walk(child, cx, st, depth + 1, out);
                    st.pop_block();
                } else {
                    alpha_walk(child, cx, st, depth + 1, out);
                }
                if cx.stopped || cx.too_deep || !st.provable {
                    return;
                }
            }
            out.push(')');
            return;
        }
        "match_arm" => {
            st.push_block();
            out.push('(');
            out.push_str(kind);
            out.push('\u{1f}');
            let pat_id = node.child_by_field_name("pattern").map(|c| c.id());
            for i in 0..node.child_count() {
                let Some(child) = node.child(i) else { continue };
                if is_trivia_token(child, kind, cx.lang) {
                    continue;
                }
                if Some(child.id()) == pat_id {
                    let mut pending = Vec::new();
                    alpha_pattern(child, cx, st, &mut pending, true, depth + 1, out);
                    for (name, id) in pending {
                        st.bind_in_current(name, id);
                    }
                } else {
                    alpha_walk(child, cx, st, depth + 1, out);
                }
                if cx.stopped || cx.too_deep || !st.provable {
                    st.pop_block();
                    return;
                }
            }
            out.push(')');
            st.pop_block();
            return;
        }
        "catch_clause" => {
            st.push_block();
            out.push('(');
            out.push_str(kind);
            out.push('\u{1f}');
            let param_id = node.child_by_field_name("parameter").map(|c| c.id());
            for i in 0..node.child_count() {
                let Some(child) = node.child(i) else { continue };
                if is_trivia_token(child, kind, cx.lang) {
                    continue;
                }
                if Some(child.id()) == param_id {
                    let mut pending = Vec::new();
                    alpha_pattern(child, cx, st, &mut pending, false, depth + 1, out);
                    for (name, id) in pending {
                        st.bind_in_current(name, id);
                    }
                } else {
                    alpha_walk(child, cx, st, depth + 1, out);
                }
                if cx.stopped || cx.too_deep || !st.provable {
                    st.pop_block();
                    return;
                }
            }
            out.push(')');
            st.pop_block();
            return;
        }
        k if is_scope_function(k) => {
            alpha_function(node, cx, st, depth, out);
            return;
        }
        _ => {}
    }

    alpha_walk_children(node, cx, st, depth, out);
}

fn alpha_let_condition(
    node: Node,
    cx: &mut Ctx,
    st: &mut ScopeState,
    pending: &mut Vec<(String, usize)>,
    depth: usize,
    out: &mut String,
) {
    out.push('(');
    out.push_str(node.kind());
    out.push('\u{1f}');
    let pat_id = node.child_by_field_name("pattern").map(|c| c.id());
    for i in 0..node.child_count() {
        let Some(child) = node.child(i) else { continue };
        if is_trivia_token(child, node.kind(), cx.lang) {
            continue;
        }
        if Some(child.id()) == pat_id {
            alpha_pattern(child, cx, st, pending, true, depth + 1, out);
        } else {
            alpha_walk(child, cx, st, depth + 1, out);
        }
        if cx.stopped || cx.too_deep || !st.provable {
            return;
        }
    }
    out.push(')');
}

fn alpha_var_declarator(
    node: Node,
    cx: &mut Ctx,
    st: &mut ScopeState,
    is_var: bool,
    depth: usize,
    out: &mut String,
) {
    out.push('(');
    out.push_str(node.kind());
    out.push('\u{1f}');
    let name_id = node.child_by_field_name("name").map(|c| c.id());
    let val_id = node.child_by_field_name("value").map(|c| c.id());
    let mut pending = Vec::new();
    for i in 0..node.child_count() {
        let Some(child) = node.child(i) else { continue };
        if is_trivia_token(child, node.kind(), cx.lang) {
            continue;
        }
        let cid = child.id();
        if Some(cid) == name_id {
            alpha_pattern(child, cx, st, &mut pending, false, depth + 1, out);
        } else if Some(cid) == val_id {
            // In JS/TS, referencing a `let`/`const` binding inside its own initializer is a TDZ ReferenceError,
            // never a read of an outer binding of the same name.
            if !is_var {
                let names: HashSet<String> = pending.iter().map(|(n, _)| n.clone()).collect();
                if contains_identifier_in(child, cx, &names, 0) {
                    st.provable = false;
                    return;
                }
            }
            alpha_walk(child, cx, st, depth + 1, out);
        } else {
            alpha_walk(child, cx, st, depth + 1, out);
        }
        if cx.stopped || cx.too_deep || !st.provable {
            return;
        }
    }
    out.push(')');
    for (name, id) in pending {
        if is_var {
            st.bind_js_var(name, id);
        } else {
            st.bind_in_current(name, id);
        }
    }
}

fn alpha_parameters(node: Node, cx: &mut Ctx, st: &mut ScopeState, depth: usize, out: &mut String) {
    out.push('(');
    out.push_str(node.kind());
    out.push('\u{1f}');
    for i in 0..node.child_count() {
        let Some(child) = node.child(i) else { continue };
        if is_trivia_token(child, node.kind(), cx.lang) {
            continue;
        }
        let ck = child.kind();
        if matches!(
            ck,
            "parameter" | "required_parameter" | "optional_parameter"
        ) {
            out.push('(');
            out.push_str(ck);
            out.push('\u{1f}');
            let pat_id = child.child_by_field_name("pattern").map(|c| c.id());
            let mut pending = Vec::new();
            for j in 0..child.child_count() {
                let Some(gc) = child.child(j) else { continue };
                if is_trivia_token(gc, ck, cx.lang) {
                    continue;
                }
                if Some(gc.id()) == pat_id {
                    alpha_pattern(gc, cx, st, &mut pending, false, depth + 2, out);
                } else {
                    alpha_walk(gc, cx, st, depth + 2, out);
                }
                if cx.stopped || cx.too_deep || !st.provable {
                    return;
                }
            }
            out.push(')');
            for (name, id) in pending {
                st.bind_in_current(name, id);
            }
        } else if ck == "self_parameter" {
            alpha_literal(child, cx, depth + 1, out);
        } else if child.is_named() {
            let mut pending = Vec::new();
            alpha_pattern(child, cx, st, &mut pending, false, depth + 1, out);
            for (name, id) in pending {
                st.bind_in_current(name, id);
            }
        } else {
            alpha_literal(child, cx, depth + 1, out);
        }
        if cx.stopped || cx.too_deep || !st.provable {
            return;
        }
    }
    out.push(')');
}

fn alpha_function(node: Node, cx: &mut Ctx, st: &mut ScopeState, depth: usize, out: &mut String) {
    let kind = node.kind();
    let is_named_expr = matches!(
        kind,
        "function_expression" | "function" | "generator_function"
    );
    let name_id = node.child_by_field_name("name").map(|c| c.id());
    let params_id = node.child_by_field_name("parameters").map(|c| c.id());
    let single_param_id = node.child_by_field_name("parameter").map(|c| c.id());
    let ret_id = node.child_by_field_name("return_type").map(|c| c.id());

    st.push_func();
    out.push('(');
    out.push_str(kind);
    out.push('\u{1f}');
    for i in 0..node.child_count() {
        let Some(child) = node.child(i) else { continue };
        if is_trivia_token(child, kind, cx.lang) {
            continue;
        }
        let cid = child.id();
        if Some(cid) == name_id {
            if is_named_expr && child.kind() == "identifier" {
                let mut pending = Vec::new();
                alpha_pattern(child, cx, st, &mut pending, false, depth + 1, out);
                for (name, id) in pending {
                    st.bind_in_current(name, id);
                }
            } else {
                alpha_literal(child, cx, depth + 1, out);
            }
        } else if Some(cid) == params_id {
            alpha_parameters(child, cx, st, depth + 1, out);
        } else if Some(cid) == single_param_id {
            let mut pending = Vec::new();
            alpha_pattern(child, cx, st, &mut pending, false, depth + 1, out);
            for (name, id) in pending {
                st.bind_in_current(name, id);
            }
        } else if Some(cid) == ret_id {
            alpha_literal(child, cx, depth + 1, out);
        } else {
            alpha_walk(child, cx, st, depth + 1, out);
        }
        if cx.stopped || cx.too_deep || !st.provable {
            st.pop_func();
            return;
        }
    }
    out.push(')');
    st.pop_func();
}

fn alpha_walk_children(
    node: Node,
    cx: &mut Ctx,
    st: &mut ScopeState,
    depth: usize,
    out: &mut String,
) {
    let parent = node.kind();
    let literal_child_id = match parent {
        "struct_item"
        | "enum_item"
        | "union_item"
        | "trait_item"
        | "mod_item"
        | "const_item"
        | "static_item"
        | "class_declaration"
        | "abstract_class_declaration"
        | "class"
        | "interface_declaration"
        | "type_alias_declaration"
        | "enum_declaration"
        | "public_field_definition"
        | "field_definition"
        | "struct_expression" => node.child_by_field_name("name").map(|c| c.id()),
        "jsx_opening_element" | "jsx_closing_element" | "jsx_self_closing_element" => node
            .child_by_field_name("name")
            .filter(|c| is_jsx_intrinsic_name(*c, cx))
            .map(|c| c.id()),
        _ => None,
    };
    out.push('(');
    out.push_str(parent);
    out.push('\u{1f}');
    for i in 0..node.child_count() {
        let Some(child) = node.child(i) else { continue };
        if is_trivia_token(child, parent, cx.lang) {
            continue;
        }
        if Some(child.id()) == literal_child_id {
            alpha_literal(child, cx, depth + 1, out);
        } else {
            alpha_walk(child, cx, st, depth + 1, out);
        }
        if cx.stopped || cx.too_deep || !st.provable {
            return;
        }
    }
    out.push(')');
}

/// The canonical form of an item with its local names replaced by lexical binding indices; `None` when a
/// rename cannot be proved for it.
fn alpha_canon(node: Node, cx: &mut Ctx) -> Option<String> {
    let mut st = ScopeState::new();
    let mut out = String::new();
    alpha_walk(node, cx, &mut st, 0, &mut out);
    if !st.provable || cx.stopped || cx.too_deep {
        return None;
    }
    Some(out)
}

/// The function that a `const f = (a) => …`, `let f = function () {…}` or class field `f = (a) => …`
/// declaration holds, when it declares one name and its value is a function: the declaration is then told by
/// its header (keyword, name, parameters, types), not by its whole text.
fn function_value(decl: Node) -> Option<Node> {
    let value = if matches!(decl.kind(), "lexical_declaration" | "variable_declaration") {
        let mut cursor = decl.walk();
        let mut declarators = decl
            .named_children(&mut cursor)
            .filter(|c| c.kind() == "variable_declarator");
        let only = declarators.next()?;
        if declarators.next().is_some() {
            return None;
        }
        only.child_by_field_name("value")?
    } else if matches!(decl.kind(), "public_field_definition" | "field_definition") {
        decl.child_by_field_name("value")?
    } else {
        return None;
    };
    matches!(
        value.kind(),
        "arrow_function" | "function_expression" | "function" | "generator_function"
    )
    .then_some(value)
}

fn body_node_for_signature(decl: Node) -> Option<Node> {
    if let Some(func) = function_value(decl) {
        return func.child_by_field_name("body");
    }
    let shape_only = matches!(
        decl.kind(),
        "struct_item"
            | "enum_item"
            | "union_item"
            | "type_item"
            | "interface_declaration"
            | "type_alias_declaration"
            | "enum_declaration"
    );
    if shape_only {
        None
    } else {
        decl.child_by_field_name("body")
    }
}

/// The canonical form of the parts of a declaration that other code depends on: everything except its body.
fn signature_canon(item_node: Node, decl: Node, cx: &mut Ctx) -> String {
    if let Some(body) = body_node_for_signature(decl) {
        header_canon(item_node, body.id(), cx)
    } else {
        canon_of(item_node, cx, &HashMap::new())
    }
}

fn collect_identifiers(node: Node, cx: &Ctx, out: &mut HashSet<String>, depth: usize) {
    if depth > MAX_DEPTH || is_comment(node) {
        return;
    }
    if node.child_count() == 0 {
        if matches!(
            node.kind(),
            "identifier" | "property_identifier" | "field_identifier" | "type_identifier"
        ) {
            out.insert(cx.text(node).to_string());
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_identifiers(child, cx, out, depth + 1);
    }
}

fn longest_increasing(seq: &[usize]) -> HashSet<usize> {
    // Positions (indices into `seq`) of one longest increasing subsequence.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![usize::MAX; seq.len()];
    for (i, &v) in seq.iter().enumerate() {
        let at = tails.partition_point(|&t| seq[t] < v);
        if at > 0 {
            prev[i] = tails[at - 1];
        }
        if at == tails.len() {
            tails.push(i);
        } else {
            tails[at] = i;
        }
    }
    let mut keep = HashSet::new();
    let mut cur = tails.last().copied();
    while let Some(i) = cur {
        keep.insert(i);
        cur = (prev[i] != usize::MAX).then_some(prev[i]);
    }
    keep
}

struct CompareState<'a> {
    cx_l: Ctx<'a>,
    cx_r: Ctx<'a>,
    changes: Vec<Change>,
}

fn compare_item_list(
    left_root: Node,
    right_root: Node,
    prefix: Option<&str>,
    inherited_export: bool,
    depth: usize,
    st: &mut CompareState<'_>,
) {
    if st.cx_l.stopped || st.cx_r.stopped || st.cx_l.too_deep || st.cx_r.too_deep {
        return;
    }
    let lang = st.cx_l.lang;
    let il = items(left_root, &mut st.cx_l, inherited_export);
    let ir = items(right_root, &mut st.cx_r, inherited_export);
    if st.cx_l.stopped || st.cx_r.stopped || st.cx_l.too_deep || st.cx_r.too_deep {
        return;
    }
    let none = HashMap::new();
    let mut matched_r: HashSet<usize> = HashSet::new();
    // Items with the same key, in order, are paired one to one.
    let mut by_key: HashMap<&str, Vec<usize>> = HashMap::new();
    for (j, it) in ir.iter().enumerate() {
        by_key.entry(it.key.as_str()).or_default().push(j);
    }
    let mut taken: HashMap<&str, usize> = HashMap::new();
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    let mut unmatched_l: Vec<usize> = Vec::new();
    for (i, it) in il.iter().enumerate() {
        let n = taken.entry(it.key.as_str()).or_default();
        match by_key.get(it.key.as_str()).and_then(|v| v.get(*n)) {
            Some(&j) => {
                *n += 1;
                matched_r.insert(j);
                pairs.push((i, j));
            }
            None => unmatched_l.push(i),
        }
    }
    let mut same_tree: Vec<(usize, usize)> = Vec::new();
    for &(i, j) in &pairs {
        if st.cx_l.stopped || st.cx_r.stopped {
            return;
        }
        let (a, b) = (&il[i], &ir[j]);
        if canon_of(a.node, &mut st.cx_l, &none) == canon_of(b.node, &mut st.cx_r, &none) {
            same_tree.push((i, j));
            continue;
        }
        // Container with an equal header (`impl`, `trait`, `class`, `mod`, `namespace`): recurse into its members.
        if depth < MAX_DEPTH {
            if let (Some(body_a), Some(body_b)) =
                (container_body(lang, a.decl), container_body(lang, b.decl))
            {
                if a.decl.kind() == b.decl.kind()
                    && header_canon(a.node, body_a.id(), &mut st.cx_l)
                        == header_canon(b.node, body_b.id(), &mut st.cx_r)
                {
                    let nested_prefix = qualify_name(prefix, a.name.clone(), lang);
                    let member_export = match a.decl.kind() {
                        "trait_item" => true,
                        "impl_item" => a.decl.child_by_field_name("trait").is_some(),
                        "class_declaration" | "abstract_class_declaration" | "class" => a.exported,
                        _ => false,
                    };
                    let before = st.changes.len();
                    compare_item_list(
                        body_a,
                        body_b,
                        nested_prefix.as_deref(),
                        member_export,
                        depth + 1,
                        st,
                    );
                    if st.cx_l.stopped || st.cx_r.stopped {
                        return;
                    }
                    if st.changes.len() == before {
                        st.changes.push(Change {
                            kind: ChangeKind::Behavior,
                            name: nested_prefix,
                            left: Some(span(a.node)),
                            right: Some(span(b.node)),
                        });
                    }
                    continue;
                }
            }
        }
        let renamed = matches!(
            (
                alpha_canon(a.node, &mut st.cx_l),
                alpha_canon(b.node, &mut st.cx_r)
            ),
            (Some(x), Some(y)) if x == y
        );
        let (left_span, right_span) = (Some(span(a.node)), Some(span(b.node)));
        let kind = if renamed {
            ChangeKind::Rename
        } else if signature_canon(a.node, a.decl, &mut st.cx_l)
            != signature_canon(b.node, b.decl, &mut st.cx_r)
        {
            ChangeKind::Signature
        } else {
            ChangeKind::Behavior
        };
        st.changes.push(Change {
            kind,
            name: qualify_name(prefix, a.name.clone(), lang),
            left: left_span,
            right: right_span,
        });
    }
    // Pairs with the same tree that stand in another order are moves.
    let mut order: Vec<(usize, usize)> = same_tree;
    order.sort();
    let rights: Vec<usize> = order.iter().map(|&(_, j)| j).collect();
    let keep = longest_increasing(&rights);
    for (pos, &(i, j)) in order.iter().enumerate() {
        if !keep.contains(&pos) {
            st.changes.push(Change {
                kind: ChangeKind::Moved,
                name: qualify_name(prefix, il[i].name.clone(), lang),
                left: Some(span(il[i].node)),
                right: Some(span(ir[j].node)),
            });
        }
    }
    // What is left: an item with another key whose tree is the same except for its own name is a renamed item
    // (a change of the public surface when it is exported); the rest is added or removed.
    let unmatched_r: Vec<usize> = (0..ir.len()).filter(|j| !matched_r.contains(j)).collect();
    if unmatched_l.is_empty() && unmatched_r.is_empty() {
        return;
    }
    // Build identifier-mention counts and stripped right-side signatures once in linear time.
    let mut ident_counts: HashMap<String, usize> = HashMap::new();
    if !unmatched_l.is_empty() {
        for it in &il {
            let mut ids = HashSet::new();
            collect_identifiers(it.node, &st.cx_l, &mut ids, 0);
            for id in ids {
                *ident_counts.entry(id).or_default() += 1;
            }
        }
    }
    let strip = |it: &Item, cx: &mut Ctx, own: &str| -> String {
        let mut m = HashMap::new();
        m.insert(own.to_string(), 0usize);
        canon_of(it.node, cx, &m)
    };
    let mut right_by_stripped: HashMap<(&str, String), VecDeque<usize>> = HashMap::new();
    for &j in &unmatched_r {
        if st.cx_r.stopped {
            return;
        }
        let b = &ir[j];
        if let Some(other) = &b.name {
            let key = (b.decl.kind(), strip(b, &mut st.cx_r, other));
            right_by_stripped.entry(key).or_default().push_back(j);
        }
    }
    let mut consumed_r: HashSet<usize> = HashSet::new();
    for i in unmatched_l {
        if st.cx_l.stopped || st.cx_r.stopped || (st.cx_l.cancel)() {
            st.cx_l.stopped = true;
            return;
        }
        let a = &il[i];
        // Another item that names this one would have to change too: then it is no rename but a removal.
        let mentioned = a
            .name
            .as_ref()
            .is_some_and(|name| ident_counts.get(name).copied().unwrap_or(0) > 1);
        let found = if let (Some(name), false) = (&a.name, mentioned) {
            let key = (a.decl.kind(), strip(a, &mut st.cx_l, name));
            right_by_stripped.get_mut(&key).and_then(|q| q.pop_front())
        } else {
            None
        };
        match found {
            Some(j) => {
                consumed_r.insert(j);
                st.changes.push(Change {
                    kind: if a.exported {
                        ChangeKind::Signature
                    } else {
                        ChangeKind::Rename
                    },
                    name: qualify_name(prefix, a.name.clone(), lang),
                    left: Some(span(a.node)),
                    right: Some(span(ir[j].node)),
                });
            }
            None => st.changes.push(Change {
                kind: ChangeKind::Removed,
                name: qualify_name(prefix, a.name.clone(), lang),
                left: Some(span(a.node)),
                right: None,
            }),
        }
    }
    for j in unmatched_r {
        if !consumed_r.contains(&j) {
            st.changes.push(Change {
                kind: ChangeKind::Added,
                name: qualify_name(prefix, ir[j].name.clone(), lang),
                left: None,
                right: Some(span(ir[j].node)),
            });
        }
    }
}

fn read_source_input(input: SourceInput) -> Result<String, Verdict> {
    match input {
        SourceInput::Text(text) => {
            if text.len() > MAX_BYTES {
                Err(Verdict::Unknown {
                    reason: Unknown::TooLarge,
                })
            } else {
                Ok(text)
            }
        }
        SourceInput::File(path) => {
            let meta = std::fs::metadata(&path).map_err(|_| Verdict::Unknown {
                reason: Unknown::Unsupported,
            })?;
            if meta.len() > MAX_BYTES as u64 {
                return Err(Verdict::Unknown {
                    reason: Unknown::TooLarge,
                });
            }
            let bytes = std::fs::read(&path).map_err(|_| Verdict::Unknown {
                reason: Unknown::Unsupported,
            })?;
            if bytes.len() > MAX_BYTES {
                return Err(Verdict::Unknown {
                    reason: Unknown::TooLarge,
                });
            }
            String::from_utf8(bytes).map_err(|_| Verdict::Unknown {
                reason: Unknown::Unsupported,
            })
        }
    }
}

/// Compares two sources provided either as raw text or as file paths on disk (reading disk files verbatim so
/// characters like CR inside string literals are preserved).
pub fn compare_sources(
    left: SourceInput,
    right: SourceInput,
    lang: Option<Language>,
    cancel: &dyn Fn() -> bool,
) -> Verdict {
    let Some(lang) = lang else {
        return Verdict::Unknown {
            reason: Unknown::Unsupported,
        };
    };
    if cancel() {
        return Verdict::Cancelled;
    }
    let left_text = match read_source_input(left) {
        Ok(t) => t,
        Err(v) => return v,
    };
    if cancel() {
        return Verdict::Cancelled;
    }
    let right_text = match read_source_input(right) {
        Ok(t) => t,
        Err(v) => return v,
    };
    compare(&left_text, &right_text, Some(lang), cancel)
}

/// Compares two sources of the same language. `cancel` is asked now and then.
pub fn compare(
    left: &str,
    right: &str,
    lang: Option<Language>,
    cancel: &dyn Fn() -> bool,
) -> Verdict {
    let Some(lang) = lang else {
        return Verdict::Unknown {
            reason: Unknown::Unsupported,
        };
    };
    if left.len() > MAX_BYTES || right.len() > MAX_BYTES {
        return Verdict::Unknown {
            reason: Unknown::TooLarge,
        };
    }
    if cancel() {
        return Verdict::Cancelled;
    }
    let mut parser = Parser::new();
    if parser.set_language(&grammar(lang)).is_err() {
        return Verdict::Unknown {
            reason: Unknown::Grammar,
        };
    }
    let (Some(tl), Some(tr)) = (parser.parse(left, None), parser.parse(right, None)) else {
        return Verdict::Unknown {
            reason: Unknown::TooLarge,
        };
    };
    for (tree, side) in [(&tl, Side::Left), (&tr, Side::Right)] {
        if tree.root_node().has_error() {
            return Verdict::Unknown {
                reason: Unknown::ParseError {
                    side,
                    line: first_error_line(tree.root_node(), 0),
                },
            };
        }
    }
    let mut st = CompareState {
        cx_l: Ctx {
            lang,
            src: left.as_bytes(),
            cancel,
            work: 0,
            stopped: false,
            too_deep: false,
        },
        cx_r: Ctx {
            lang,
            src: right.as_bytes(),
            cancel,
            work: 0,
            stopped: false,
            too_deep: false,
        },
        changes: Vec::new(),
    };
    let (cl, cr) = (
        file_canon(tl.root_node(), &mut st.cx_l),
        file_canon(tr.root_node(), &mut st.cx_r),
    );
    if st.cx_l.stopped || st.cx_r.stopped {
        return Verdict::Cancelled;
    }
    if st.cx_l.too_deep || st.cx_r.too_deep {
        return Verdict::Unknown {
            reason: Unknown::TooLarge,
        };
    }
    if cl == cr {
        return Verdict::Equivalent {
            evidence: if left == right {
                Evidence::Identical
            } else {
                Evidence::SameTree
            },
        };
    }

    compare_item_list(tl.root_node(), tr.root_node(), None, false, 0, &mut st);
    if st.cx_l.stopped || st.cx_r.stopped {
        return Verdict::Cancelled;
    }
    if st.cx_l.too_deep || st.cx_r.too_deep {
        return Verdict::Unknown {
            reason: Unknown::TooLarge,
        };
    }
    if st.changes.is_empty() {
        // The files differ but no item does: the order of items that cannot be moved (JavaScript imports) or
        // statements. Nothing can be claimed.
        st.changes.push(Change {
            kind: ChangeKind::Moved,
            name: None,
            left: Some(span(tl.root_node())),
            right: Some(span(tr.root_node())),
        });
    }
    st.changes.sort_by_key(|c| {
        (
            c.left.map_or(usize::MAX, |s| s.start),
            c.right.map_or(usize::MAX, |s| s.start),
        )
    });
    Verdict::Different {
        changes: st.changes,
    }
}

/// Most nodes of an outline; a larger file is cut here (the rest is simply not listed).
pub const MAX_OUTLINE_NODES: usize = 20_000;

/// A node of the structural outline: a module, class, struct, function, … with where it stands in the text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineNode {
    /// `module`, `class`, `struct`, `enum`, `trait`, `impl`, `interface`, `function`, `method`, `field`, `closure`, `macro`.
    pub kind: &'static str,
    /// `None` for an anonymous function or closure that no name is given to.
    pub name: Option<String>,
    /// Where the node starts (byte range of the whole node and its first line).
    pub span: Span,
    /// The last line of the node (1-based), so that a change can be linked to it by line.
    pub end_line: usize,
    pub children: Vec<OutlineNode>,
}

fn outline_label(lang: Language, node: Node, parent: &str) -> Option<&'static str> {
    let kind = node.kind();
    match lang {
        Language::Rust => match kind {
            "mod_item" => Some("module"),
            "struct_item" | "union_item" => Some("struct"),
            "enum_item" => Some("enum"),
            "trait_item" => Some("trait"),
            "impl_item" => Some("impl"),
            // A method that a trait only declares (`fn name(&self);`) is listed beside the ones it defines.
            "function_signature_item" if parent == "trait" => Some("method"),
            "function_item" => Some(if matches!(parent, "impl" | "trait") {
                "method"
            } else {
                "function"
            }),
            "closure_expression" => Some("closure"),
            "macro_definition" => Some("macro"),
            _ => None,
        },
        Language::JavaScript | Language::TypeScript | Language::Tsx => match kind {
            "class_declaration" | "class" | "abstract_class_declaration" => Some("class"),
            "method_definition" | "method_signature" => Some("method"),
            // A field of a class (`x = 1;`, `private y: number;`), also when it holds an arrow function.
            "public_field_definition" | "field_definition" if parent == "class" => Some("field"),
            "function_declaration"
            | "generator_function_declaration"
            | "function_expression"
            | "function"
            | "generator_function"
            | "arrow_function" => Some("function"),
            "interface_declaration" => Some("interface"),
            "enum_declaration" => Some("enum"),
            "internal_module" | "module" => Some("module"),
            _ => None,
        },
    }
}

/// The name of an outline node: its own, or for an anonymous function the name it is assigned to.
fn outline_name(cx: &Ctx, node: Node) -> Option<String> {
    if node.kind() == "impl_item" {
        let ty = node
            .child_by_field_name("type")
            .map(|n| cx.text(n).to_string())?;
        return Some(match node.child_by_field_name("trait") {
            Some(tr) => format!("{} for {ty}", cx.text(tr)),
            None => ty,
        });
    }
    if let Some(n) = node
        .child_by_field_name("name")
        .or_else(|| node.child_by_field_name("property"))
    {
        let t = cx.text(n);
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    let parent = node.parent()?;
    let named = match parent.kind() {
        "variable_declarator" => parent.child_by_field_name("name"),
        "assignment_expression" => parent.child_by_field_name("left"),
        "pair" => parent.child_by_field_name("key"),
        "public_field_definition" | "field_definition" => parent.child_by_field_name("name"),
        "let_declaration" => parent.child_by_field_name("pattern"),
        _ => None,
    }?;
    Some(cx.text(named).to_string())
}

fn outline_walk(
    node: Node,
    cx: &mut Ctx,
    parent: &str,
    out: &mut Vec<OutlineNode>,
    depth: usize,
    budget: &mut usize,
    cut: &mut bool,
) {
    if depth > MAX_DEPTH || *budget == 0 {
        // Nothing more is listed below: say so when there was something below.
        if node.named_child_count() > 0 {
            *cut = true;
        }
        return;
    }
    cx.tick();
    if cx.stopped {
        return;
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.is_extra() {
            continue;
        }
        if let Some(label) = outline_label(cx.lang, child, parent) {
            if *budget == 0 {
                *cut = true;
                return;
            }
            *budget -= 1;
            let mut item = OutlineNode {
                kind: label,
                name: outline_name(cx, child),
                span: span(child),
                end_line: child.end_position().row + 1,
                children: Vec::new(),
            };
            outline_walk(child, cx, label, &mut item.children, depth + 1, budget, cut);
            out.push(item);
        } else {
            outline_walk(child, cx, parent, out, depth + 1, budget, cut);
        }
    }
}

/// The structural outline of a source: nested modules, classes, structs and functions (anonymous ones named by
/// what they are assigned to, else `None`). Unsupported languages and text that does not parse cleanly give an
/// error, never a guess.
pub fn outline(
    text: &str,
    lang: Option<Language>,
    cancel: &dyn Fn() -> bool,
) -> Result<Vec<OutlineNode>, Verdict> {
    outline_cut(text, lang, cancel).map(|o| o.nodes)
}

/// The nodes of an outline, and whether the list was cut at [`MAX_OUTLINE_NODES`] (the rest is not listed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outline {
    pub nodes: Vec<OutlineNode>,
    pub truncated: bool,
}

/// [`outline`] that also tells when the list was cut short.
pub fn outline_cut(
    text: &str,
    lang: Option<Language>,
    cancel: &dyn Fn() -> bool,
) -> Result<Outline, Verdict> {
    outline_cut_of(text, lang, Side::Left, cancel)
}

/// [`outline_cut`] for the text of one side of a comparison: a parse error names that side.
pub fn outline_cut_of(
    text: &str,
    lang: Option<Language>,
    side: Side,
    cancel: &dyn Fn() -> bool,
) -> Result<Outline, Verdict> {
    let Some(lang) = lang else {
        return Err(Verdict::Unknown {
            reason: Unknown::Unsupported,
        });
    };
    if text.len() > MAX_BYTES {
        return Err(Verdict::Unknown {
            reason: Unknown::TooLarge,
        });
    }
    if cancel() {
        return Err(Verdict::Cancelled);
    }
    let mut parser = Parser::new();
    if parser.set_language(&grammar(lang)).is_err() {
        return Err(Verdict::Unknown {
            reason: Unknown::Grammar,
        });
    }
    let Some(tree) = parser.parse(text, None) else {
        return Err(Verdict::Unknown {
            reason: Unknown::TooLarge,
        });
    };
    if tree.root_node().has_error() {
        return Err(Verdict::Unknown {
            reason: Unknown::ParseError {
                side,
                line: first_error_line(tree.root_node(), 0),
            },
        });
    }
    let mut cx = Ctx {
        lang,
        src: text.as_bytes(),
        cancel,
        work: 0,
        stopped: false,
        too_deep: false,
    };
    let mut out = Vec::new();
    let mut budget = MAX_OUTLINE_NODES;
    let mut truncated = false;
    outline_walk(
        tree.root_node(),
        &mut cx,
        "",
        &mut out,
        0,
        &mut budget,
        &mut truncated,
    );
    if cx.stopped {
        return Err(Verdict::Cancelled);
    }
    Ok(Outline {
        nodes: out,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(lang: Language, a: &str, b: &str) -> Verdict {
        compare(a, b, Some(lang), &|| false)
    }

    fn kinds(v: &Verdict) -> Vec<ChangeKind> {
        match v {
            Verdict::Different { changes } => changes.iter().map(|c| c.kind).collect(),
            other => panic!("expected a difference, got {other:?}"),
        }
    }

    #[test]
    fn tsx_is_parsed_with_its_own_grammar_so_a_jsx_tag_is_not_an_error() {
        let pretty = "const A = () => {\n  return <div className=\"a\">{x}</div>;\n};\n";
        let compact = "const A=()=>{return <div className=\"a\">{x}</div>};";
        assert!(matches!(
            run(Language::Tsx, pretty, compact),
            Verdict::Equivalent { .. }
        ));
        let changed = "const A = () => {\n  return <div className=\"b\">{x}</div>;\n};\n";
        assert!(!kinds(&run(Language::Tsx, pretty, changed)).is_empty());
        // The TypeScript grammar does not read the tag: the comparison is unknown there, not a verdict.
        assert!(matches!(
            run(Language::TypeScript, pretty, compact),
            Verdict::Unknown { .. }
        ));
    }

    #[test]
    fn languages_by_extension_and_unsupported_is_unknown() {
        assert_eq!(language_of("a.RS"), Some(Language::Rust));
        assert_eq!(language_of("b.mjs"), Some(Language::JavaScript));
        assert_eq!(language_of("c.ts"), Some(Language::TypeScript));
        assert_eq!(language_of("App.TSX"), Some(Language::Tsx));
        assert_eq!(language_of("d.py"), None);
        assert_eq!(
            compare("x", "y", None, &|| false),
            Verdict::Unknown {
                reason: Unknown::Unsupported
            }
        );
    }

    #[test]
    fn identical_text_is_identical() {
        let v = run(Language::Rust, "fn a() {}\n", "fn a() {}\n");
        assert_eq!(
            v,
            Verdict::Equivalent {
                evidence: Evidence::Identical
            }
        );
    }

    #[test]
    fn minified_and_pretty_javascript_are_the_same_tree() {
        let pretty = "function add(a, b) {\n  // sum\n  return a + b;\n}\nconst xs = [1, 2, 3,];\n";
        let minified = "function add(a,b){return a+b}const xs=[1,2,3];";
        let v = run(Language::JavaScript, pretty, minified);
        assert_eq!(
            v,
            Verdict::Equivalent {
                evidence: Evidence::SameTree
            }
        );
    }

    #[test]
    fn comments_and_layout_in_rust_are_trivia_but_a_semicolon_is_not() {
        let a = "fn f() -> u32 {\n    // one\n    let x = 1;\n    x\n}\n";
        let b = "fn f() -> u32 { /* one */ let x = 1; x }";
        assert_eq!(
            run(Language::Rust, a, b),
            Verdict::Equivalent {
                evidence: Evidence::SameTree
            }
        );
        // `x` and `x;` are different: the first returns the value.
        let c = "fn f() -> u32 { let x = 1; x; }";
        assert!(matches!(
            run(Language::Rust, a, c),
            Verdict::Different { .. } | Verdict::Unknown { .. }
        ));
    }

    #[test]
    fn a_trailing_comma_is_trivia_except_in_a_one_element_tuple() {
        let a = "fn f() { g(1, 2,); let v = [1, 2,]; }";
        let b = "fn f() { g(1, 2); let v = [1, 2]; }";
        assert_eq!(
            run(Language::Rust, a, b),
            Verdict::Equivalent {
                evidence: Evidence::SameTree
            }
        );
        let t1 = "fn f() { let t = (1,); }";
        let t2 = "fn f() { let t = (1); }";
        assert!(matches!(
            run(Language::Rust, t1, t2),
            Verdict::Different { .. }
        ));
        // A hole in a JavaScript array is an element.
        assert!(matches!(
            run(Language::JavaScript, "const a = [1,,];", "const a = [1,];"),
            Verdict::Different { .. }
        ));
    }

    #[test]
    fn asi_hazard_is_a_different_tree() {
        // Without the semicolon the next line continues the expression: not the same program.
        let with = "const a = b;\n(c || d).run();\n";
        let without = "const a = b\n(c || d).run()\n";
        assert!(matches!(
            run(Language::JavaScript, with, without),
            Verdict::Different { .. }
        ));
        // Where nothing continues, the semicolon is only a terminator.
        let safe_a = "const a = b;\nconst c = d;\n";
        let safe_b = "const a = b\nconst c = d\n";
        assert_eq!(
            run(Language::JavaScript, safe_a, safe_b),
            Verdict::Equivalent {
                evidence: Evidence::SameTree
            }
        );
    }

    #[test]
    fn rust_use_order_is_ignored_but_javascript_import_order_is_not() {
        let a = "use std::fs;\nuse std::io;\nfn f() {}\n";
        let b = "use std::io;\nuse std::fs;\nfn f() {}\n";
        assert_eq!(
            run(Language::Rust, a, b),
            Verdict::Equivalent {
                evidence: Evidence::SameTree
            }
        );
        let ja = "import './polyfill';\nimport { x } from './x';\n";
        let jb = "import { x } from './x';\nimport './polyfill';\n";
        let v = run(Language::JavaScript, ja, jb);
        assert!(matches!(v, Verdict::Different { .. }), "{v:?}");
        assert!(kinds(&v).contains(&ChangeKind::Moved));
    }

    #[test]
    fn consistent_local_rename_is_a_rename() {
        let a = "fn total(items: &[u32]) -> u32 {\n    let mut sum = 0;\n    for i in items { sum += i; }\n    sum\n}\n";
        let b = "fn total(items: &[u32]) -> u32 {\n    let mut acc = 0;\n    for n in items { acc += n; }\n    acc\n}\n";
        assert_eq!(kinds(&run(Language::Rust, a, b)), vec![ChangeKind::Rename]);
    }

    #[test]
    fn shadowing_capture_and_macros_are_not_proved_as_renames() {
        // Valid shadowing rename: the second `let x` reads the renamed first binding consistently.
        let a = "fn f() -> u32 { let x = 1; let x = x + 1; x }";
        let b = "fn f() -> u32 { let y = 1; let x = y + 1; x }";
        assert_eq!(kinds(&run(Language::Rust, a, b)), vec![ChangeKind::Rename]);
        // Broken shadowing: inner `let x = 2` now shadows the outer `x` read by the block.
        let bad_a = "fn f() -> u32 { let x = 1; let r = { let y = 2; x }; r }";
        let bad_b = "fn f() -> u32 { let x = 1; let r = { let x = 2; x }; r }";
        assert_eq!(
            kinds(&run(Language::Rust, bad_a, bad_b)),
            vec![ChangeKind::Behavior]
        );
        // Capture: the new name is a global that the body also uses.
        let ca = "fn f(g: u32) -> u32 { let x = 1; x + G }";
        let cb = "fn f(g: u32) -> u32 { let G = 1; G + G }";
        assert_eq!(
            kinds(&run(Language::Rust, ca, cb)),
            vec![ChangeKind::Behavior]
        );
        // A name used inside macro arguments is opaque to the tree.
        let ma = "fn f() { let x = 1; println!(\"{}\", x); }";
        let mb = "fn f() { let y = 1; println!(\"{}\", y); }";
        assert_eq!(
            kinds(&run(Language::Rust, ma, mb)),
            vec![ChangeKind::Behavior]
        );
        // A shorthand property or struct field initializer is also a field/property key.
        let sa = "function f() { const x = 1; return { x }; }";
        let sb = "function f() { const y = 1; return { y }; }";
        assert_eq!(
            kinds(&run(Language::JavaScript, sa, sb)),
            vec![ChangeKind::Behavior]
        );
        let rsa = "fn f() -> S { let x = 1; S { x } }";
        let rsb = "fn f() -> S { let y = 1; S { y } }";
        assert_eq!(
            kinds(&run(Language::Rust, rsa, rsb)),
            vec![ChangeKind::Behavior]
        );
    }

    #[test]
    fn recursive_members_inside_impl_trait_class_and_mod() {
        let a = "mod m {\n    impl S {\n        fn a(&self) -> u32 { 1 }\n        fn b(&self) -> u32 { 2 }\n    }\n}\n";
        let b = "mod m {\n    impl S {\n        fn a(&self) -> u32 { 1 }\n        fn b(&self) -> u32 { 3 }\n    }\n}\n";
        let Verdict::Different { changes } = run(Language::Rust, a, b) else {
            panic!("expected different");
        };
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Behavior);
        assert_eq!(changes[0].name.as_deref(), Some("m::S::b"));
        assert_eq!(changes[0].left.unwrap().line, 4);
        assert_eq!(changes[0].right.unwrap().line, 4);

        // Two distinct impl blocks for the same type pair by header, not by collision.
        let ia = "impl Foo {\n    fn a(&self) {}\n}\nimpl Bar for Foo {\n    fn fmt(&self) -> u32 { 1 }\n}\n";
        let ib = "impl Foo {\n    fn a(&self) {}\n}\nimpl Bar for Foo {\n    fn fmt(&self) -> u32 { 2 }\n}\n";
        let Verdict::Different { changes } = run(Language::Rust, ia, ib) else {
            panic!("expected different");
        };
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Behavior);
        assert_eq!(changes[0].name.as_deref(), Some("Bar for Foo::fmt"));

        // Getter and setter with the same property name do not collide.
        let ca = "class C {\n  get x() { return 1; }\n  set x(v) { this._x = v; }\n}\n";
        let cb = "class C {\n  get x() { return 1; }\n  set x(v) { this._x = v + 1; }\n}\n";
        let Verdict::Different { changes } = run(Language::JavaScript, ca, cb) else {
            panic!("expected different");
        };
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Behavior);
        assert_eq!(changes[0].name.as_deref(), Some("C.x"));
        assert_eq!(changes[0].left.unwrap().line, 3);
    }

    #[test]
    fn nested_scopes_closures_and_js_guards() {
        // Two closures in the same item using the same parameter name.
        let ra = "fn f(a: &[u32], b: &[u32]) -> usize { a.iter().map(|&x| x + 1).count() + b.iter().map(|x: &u32| *x).count() }";
        let rb = "fn f(a: &[u32], b: &[u32]) -> usize { a.iter().map(|&n| n + 1).count() + b.iter().map(|m: &u32| *m).count() }";
        assert_eq!(
            kinds(&run(Language::Rust, ra, rb)),
            vec![ChangeKind::Rename]
        );

        // Match pattern uppercase variant `None` is not renamed as a variable binding.
        let ma = "fn f(o: Option<u32>) -> u32 { match o { Some(x) => x, None => 0 } }";
        let mb = "fn f(o: Option<u32>) -> u32 { match o { Some(n) => n, None => 0 } }";
        assert_eq!(
            kinds(&run(Language::Rust, ma, mb)),
            vec![ChangeKind::Rename]
        );
        let mc = "fn f(o: Option<u32>) -> u32 { match o { Some(x) => x, Other => 0 } }";
        assert_eq!(
            kinds(&run(Language::Rust, ma, mc)),
            vec![ChangeKind::Behavior]
        );

        // JS `let` TDZ self-reference is refused as a rename.
        let ja = "function f() { let x = 1; { let x = x + 1; return x; } }";
        let jb = "function f() { let a = 1; { let b = a + 1; return b; } }";
        assert_eq!(
            kinds(&run(Language::JavaScript, ja, jb)),
            vec![ChangeKind::Behavior]
        );

        // JS nested `var` hoisting is refused conservatively.
        let va = "function f(c) { if (c) { var x = 1; } return x; }";
        let vb = "function f(c) { if (c) { var y = 1; } return y; }";
        assert_eq!(
            kinds(&run(Language::JavaScript, va, vb)),
            vec![ChangeKind::Behavior]
        );
    }

    #[test]
    fn disk_files_preserve_cr_inside_string_literals() {
        let dir = tempfile::tempdir().unwrap();
        // Rust string literal with literal CR vs without.
        let left_rs = dir.path().join("a.rs");
        let right_rs = dir.path().join("b.rs");
        std::fs::write(&left_rs, b"const S: &str = \"a\rb\";\n").unwrap();
        std::fs::write(&right_rs, b"const S: &str = \"ab\";\n").unwrap();
        let v = compare_sources(
            SourceInput::File(left_rs),
            SourceInput::File(right_rs),
            language_of("a.rs"),
            &|| false,
        );
        assert!(matches!(v, Verdict::Different { .. }), "{v:?}");

        // JS template literal with CRLF vs LF inside the literal.
        let left_js = dir.path().join("a.js");
        let right_js = dir.path().join("b.js");
        std::fs::write(&left_js, b"const s = `a\r\nb`;\n").unwrap();
        std::fs::write(&right_js, b"const s = `a\nb`;\n").unwrap();
        let v_js = compare_sources(
            SourceInput::File(left_js),
            SourceInput::File(right_js),
            language_of("a.js"),
            &|| false,
        );
        assert!(matches!(v_js, Verdict::Different { .. }), "{v_js:?}");
    }

    #[test]
    fn trailing_comma_with_comment_and_array_hole_with_comment() {
        let a = "const xs = [1, /* keep */ ];";
        let b = "const xs = [1];";
        assert_eq!(
            run(Language::JavaScript, a, b),
            Verdict::Equivalent {
                evidence: Evidence::SameTree
            }
        );
        let hole = "const xs = [1, /* hole */ ,];";
        assert!(matches!(
            run(Language::JavaScript, hole, b),
            Verdict::Different { .. }
        ));
    }

    #[test]
    fn t01_scale_input_is_bounded_and_over_limit_rejects_immediately() {
        // 40,000-line Rust source (~1.4 MiB, matching the app_speed diff fixture scale) with a change inside an impl.
        let mut left = String::with_capacity(1_600_000);
        let mut right = String::with_capacity(1_600_000);
        left.push_str("struct S;\nimpl S {\n");
        right.push_str("struct S;\nimpl S {\n");
        for i in 0..8_000 {
            let line = format!("    fn m_{i}(&self, x: u32) -> u32 {{\n        let v = x + {i};\n        let w = v * 2;\n        w\n    }}\n");
            left.push_str(&line);
            if i == 4_000 {
                right.push_str(&line.replace("* 2", "* 3"));
            } else {
                right.push_str(&line);
            }
        }
        left.push_str("}\n");
        right.push_str("}\n");

        let t0 = std::time::Instant::now();
        let v = compare(&left, &right, Some(Language::Rust), &|| false);
        let elapsed = t0.elapsed();
        match v {
            Verdict::Different { changes } => {
                assert_eq!(changes.len(), 1);
                assert_eq!(changes[0].kind, ChangeKind::Behavior);
                assert_eq!(changes[0].name.as_deref(), Some("S::m_4000"));
            }
            other => panic!("expected member behavior change, got {other:?}"),
        }
        // Even in unoptimized debug builds, 40,000 lines should stay well under 15 seconds.
        assert!(
            elapsed.as_secs() < 15,
            "T01-scale semantic compare took too long: {elapsed:?}"
        );

        // Over MAX_BYTES (200,000-line T01 ceiling) returns TooLarge immediately without parsing.
        let huge = "fn a() {}\n".repeat(MAX_BYTES / 9 + 10);
        let t1 = std::time::Instant::now();
        let over = compare(&huge, &huge, Some(Language::Rust), &|| false);
        assert_eq!(
            over,
            Verdict::Unknown {
                reason: Unknown::TooLarge
            }
        );
        assert!(t1.elapsed().as_millis() < 50);
    }

    #[test]
    fn an_empty_array_is_not_a_hole_and_other_audit_cases() {
        assert!(matches!(
            run(Language::JavaScript, "const a = [];", "const a = [,];"),
            Verdict::Different { .. }
        ));
        // `x` is read before it is declared: it may be another binding, so no rename is proved.
        let a = "fn f() -> u32 { g(x); let x = 1; x }";
        let b = "fn f() -> u32 { g(y); let y = 1; y }";
        assert_eq!(
            kinds(&run(Language::Rust, a, b)),
            vec![ChangeKind::Behavior]
        );
        // A default value is an expression, not a binding.
        let da = "function f(a = g) { return a; }";
        let db = "function f(a = h) { return a; }";
        // (the default value is part of the signature)
        assert_eq!(
            kinds(&run(Language::JavaScript, da, db)),
            vec![ChangeKind::Signature]
        );
        // A renamed item that another item calls is no rename.
        let ca = "fn helper() -> u32 { 1 }\nfn main() { helper(); }\n";
        let cb = "fn helper2() -> u32 { 1 }\nfn main() { helper(); }\n";
        let ks = kinds(&run(Language::Rust, ca, cb));
        assert!(!ks.contains(&ChangeKind::Rename), "{ks:?}");
        // A placeholder-looking name cannot imitate a renamed local.
        let pa = "function f(x) { return x; }";
        let pb = "function f(a) { return $0; }";
        assert_eq!(
            kinds(&run(Language::JavaScript, pa, pb)),
            vec![ChangeKind::Signature]
        );
        // The field of a struct is its shape.
        let sa = "struct S { a: u32 }";
        let sb = "struct S { a: u64 }";
        assert_eq!(
            kinds(&run(Language::Rust, sa, sb)),
            vec![ChangeKind::Signature]
        );
        // An attribute stays with its `use`.
        let ua = "#[cfg(unix)]\nuse a::P;\nuse std::fs;\n";
        let ub = "#[cfg(unix)]\nuse std::fs;\nuse a::P;\n";
        assert!(matches!(
            run(Language::Rust, ua, ub),
            Verdict::Different { .. }
        ));
        assert_eq!(language_of("js"), None);
    }

    #[test]
    fn closure_capture_in_javascript() {
        let a = "function f(a) { return () => a + 1; }";
        let b = "function f(b) { return () => b + 1; }";
        assert_eq!(
            kinds(&run(Language::JavaScript, a, b)),
            vec![ChangeKind::Rename]
        );
        let c = "function f(a) { return () => z + 1; }";
        assert_eq!(
            kinds(&run(Language::JavaScript, a, c)),
            vec![ChangeKind::Behavior]
        );
    }

    #[test]
    fn signature_body_move_added_removed_are_told_apart() {
        let a = "fn one(x: u32) -> u32 { x }\nfn two() {}\nfn gone() { 1 }\n";
        // `one` got a parameter, `two` is the same but moved below, `gone` is removed, `fresh` is added.
        let b = "fn one(x: u32, y: u32) -> u32 { x }\nfn fresh() { 2 }\nfn two() {}\n";
        let ks = kinds(&run(Language::Rust, a, b));
        assert!(ks.contains(&ChangeKind::Signature), "{ks:?}");
        assert!(
            ks.contains(&ChangeKind::Removed) && ks.contains(&ChangeKind::Added),
            "{ks:?}"
        );
        let body_a = "fn f() -> u32 { 1 }";
        let body_b = "fn f() -> u32 { 2 }";
        assert_eq!(
            kinds(&run(Language::Rust, body_a, body_b)),
            vec![ChangeKind::Behavior]
        );
        let ma = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let mb = "fn b() {}\nfn c() {}\nfn a() {}\n";
        assert_eq!(kinds(&run(Language::Rust, ma, mb)), vec![ChangeKind::Moved]);
    }

    #[test]
    fn an_exported_item_that_is_renamed_is_a_signature_change() {
        let a = "pub fn old_name() -> u32 { 1 }";
        let b = "pub fn new_name() -> u32 { 1 }";
        assert_eq!(
            kinds(&run(Language::Rust, a, b)),
            vec![ChangeKind::Signature]
        );
        let pa = "fn old_name() -> u32 { 1 }";
        let pb = "fn new_name() -> u32 { 1 }";
        assert_eq!(
            kinds(&run(Language::Rust, pa, pb)),
            vec![ChangeKind::Rename]
        );
        let ja = "export function oldName() { return 1; }";
        let jb = "export function newName() { return 1; }";
        assert_eq!(
            kinds(&run(Language::JavaScript, ja, jb)),
            vec![ChangeKind::Signature]
        );
    }

    #[test]
    fn partial_parses_are_unknown_with_the_place() {
        let v = run(Language::Rust, "fn ok() {}\n", "fn broken( {\n");
        assert!(
            matches!(
                v,
                Verdict::Unknown {
                    reason: Unknown::ParseError {
                        side: Side::Right,
                        line: 1
                    }
                }
            ),
            "{v:?}"
        );
        let v = run(Language::JavaScript, "function f( {", "function f() {}");
        assert!(
            matches!(
                v,
                Verdict::Unknown {
                    reason: Unknown::ParseError {
                        side: Side::Left,
                        ..
                    }
                }
            ),
            "{v:?}"
        );
    }

    #[test]
    fn changes_keep_byte_ranges_of_the_original() {
        let a = "fn a() { 1 }\nfn b() { 2 }\n";
        let b = "fn a() { 1 }\nfn b() { 3 }\n";
        let Verdict::Different { changes } = run(Language::Rust, a, b) else {
            panic!("difference")
        };
        assert_eq!(changes.len(), 1);
        let (l, r) = (changes[0].left.unwrap(), changes[0].right.unwrap());
        assert_eq!(&a[l.start..l.end], "fn b() { 2 }");
        assert_eq!(&b[r.start..r.end], "fn b() { 3 }");
        assert_eq!((l.line, r.line), (2, 2));
    }

    #[test]
    fn cancel_and_limits() {
        assert_eq!(
            compare("fn a() {}", "fn a() {}", Some(Language::Rust), &|| true),
            Verdict::Cancelled
        );
        let big = " ".repeat(MAX_BYTES + 1);
        assert_eq!(
            compare(&big, "", Some(Language::Rust), &|| false),
            Verdict::Unknown {
                reason: Unknown::TooLarge
            }
        );
        let deep = format!("fn f() {{ {}1{} }}", "(".repeat(2000), ")".repeat(2000));
        let v = compare(
            &deep,
            &deep.replace('1', "2"),
            Some(Language::Rust),
            &|| false,
        );
        assert!(matches!(v, Verdict::Unknown { .. }), "{v:?}");
    }

    fn names(nodes: &[OutlineNode]) -> Vec<String> {
        nodes
            .iter()
            .map(|n| {
                format!(
                    "{} {}",
                    n.kind,
                    n.name.clone().unwrap_or_else(|| "?".into())
                )
            })
            .collect()
    }

    #[test]
    fn an_outline_parse_error_names_the_side_of_the_text() {
        for side in [Side::Left, Side::Right] {
            match outline_cut_of("fn broken( {", Some(Language::Rust), side, &|| false) {
                Err(Verdict::Unknown {
                    reason: Unknown::ParseError { side: got, .. },
                }) => assert_eq!(got, side),
                other => panic!("expected a parse error, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_outline_cut_at_the_limit_says_so() {
        let few = outline_cut("fn a() {}\nfn b() {}\n", Some(Language::Rust), &|| false).unwrap();
        assert_eq!(few.nodes.len(), 2);
        assert!(!few.truncated);
        let many: String = (0..MAX_OUTLINE_NODES + 50)
            .map(|i| format!("fn f{i}() {{}}\n"))
            .collect();
        let cut = outline_cut(&many, Some(Language::Rust), &|| false).unwrap();
        assert_eq!(cut.nodes.len(), MAX_OUTLINE_NODES);
        assert!(cut.truncated);
        // `outline` keeps its shape for the callers that do not need the flag.
        assert_eq!(
            outline(&many, Some(Language::Rust), &|| false)
                .unwrap()
                .len(),
            MAX_OUTLINE_NODES
        );
    }

    #[test]
    fn the_outline_lists_trait_declarations_and_class_fields() {
        let rust = "trait T {\n    fn declared(&self);\n    fn defined(&self) {}\n}\n";
        let o = outline(rust, Some(Language::Rust), &|| false).unwrap();
        let methods: Vec<(&str, Option<&str>)> = o[0]
            .children
            .iter()
            .map(|c| (c.kind, c.name.as_deref()))
            .collect();
        assert_eq!(
            methods,
            [("method", Some("declared")), ("method", Some("defined"))]
        );

        let js = "class A {\n  x = 1;\n  handler = () => {};\n  run() {}\n}\n";
        let o = outline(js, Some(Language::JavaScript), &|| false).unwrap();
        let members: Vec<(&str, Option<&str>)> = o[0]
            .children
            .iter()
            .map(|c| (c.kind, c.name.as_deref()))
            .collect();
        assert_eq!(
            members,
            [
                ("field", Some("x")),
                ("field", Some("handler")),
                ("method", Some("run"))
            ]
        );
        let ts = "class B {\n  private y: number = 2;\n  m(): void {}\n}\n";
        let o = outline(ts, Some(Language::TypeScript), &|| false).unwrap();
        let members: Vec<(&str, Option<&str>)> = o[0]
            .children
            .iter()
            .map(|c| (c.kind, c.name.as_deref()))
            .collect();
        assert_eq!(members, [("field", Some("y")), ("method", Some("m"))]);
        // A field outside a class (an object literal's pair) is not a field of the outline.
        let o = outline("const o = { a: 1 };\n", Some(Language::JavaScript), &|| {
            false
        })
        .unwrap();
        assert!(o.is_empty(), "{o:?}");
    }

    #[test]
    fn rust_outline_nests_modules_impls_and_methods() {
        let src = "mod m {\n    pub struct S;\n    impl S {\n        fn a(&self) {}\n        fn b(&self) { let f = |x| x; }\n    }\n}\nfn top() {}\ntrait T { fn t(&self); }\n";
        let o = outline(src, Some(Language::Rust), &|| false).unwrap();
        assert_eq!(names(&o), ["module m", "function top", "trait T"]);
        assert_eq!(names(&o[0].children), ["struct S", "impl S"]);
        assert_eq!(names(&o[0].children[1].children), ["method a", "method b"]);
        // The closure inside `b` is a nested anonymous node named by its binding.
        assert_eq!(names(&o[0].children[1].children[1].children), ["closure f"]);
        assert_eq!((o[0].span.line, o[0].end_line), (1, 7));
        assert_eq!(o[1].span.line, 8);
    }

    #[test]
    fn javascript_outline_names_anonymous_functions_by_assignment() {
        let src = "class A { m() {} static n() {} }\nconst f = () => 1;\nconst g = function () {};\nexport function h() { return () => 2; }\nsetTimeout(function () {}, 1);\n";
        let o = outline(src, Some(Language::JavaScript), &|| false).unwrap();
        let top = names(&o);
        assert!(top.contains(&"class A".to_string()), "{top:?}");
        assert!(top.contains(&"function f".to_string()), "{top:?}");
        assert!(top.contains(&"function g".to_string()), "{top:?}");
        assert!(top.contains(&"function h".to_string()), "{top:?}");
        // Passed straight as an argument: anonymous.
        assert!(
            o.iter().any(|n| n.kind == "function" && n.name.is_none()),
            "{top:?}"
        );
        assert_eq!(names(&o[0].children), ["method m", "method n"]);
        // The arrow inside `h` is its child and anonymous.
        let h = o.iter().find(|n| n.name.as_deref() == Some("h")).unwrap();
        assert_eq!(h.children.len(), 1);
        assert!(h.children[0].name.is_none());
    }

    #[test]
    fn typescript_outline_has_interfaces_and_enums() {
        let src =
            "interface I { a(): void }\nenum E { A, B }\nnamespace N { export function f() {} }\n";
        let o = outline(src, Some(Language::TypeScript), &|| false).unwrap();
        let top = names(&o);
        assert!(
            top.contains(&"interface I".to_string()) && top.contains(&"enum E".to_string()),
            "{top:?}"
        );
        assert!(top.iter().any(|t| t.starts_with("module")), "{top:?}");
    }

    #[test]
    fn outline_refuses_what_it_cannot_read() {
        assert_eq!(
            outline("x", None, &|| false),
            Err(Verdict::Unknown {
                reason: Unknown::Unsupported
            })
        );
        let e = outline("fn broken( {", Some(Language::Rust), &|| false).unwrap_err();
        assert!(
            matches!(
                e,
                Verdict::Unknown {
                    reason: Unknown::ParseError { .. }
                }
            ),
            "{e:?}"
        );
        assert_eq!(
            outline("fn a() {}", Some(Language::Rust), &|| true).unwrap_err(),
            Verdict::Cancelled
        );
        assert!(outline("", Some(Language::Rust), &|| false)
            .unwrap()
            .is_empty());
    }
}
