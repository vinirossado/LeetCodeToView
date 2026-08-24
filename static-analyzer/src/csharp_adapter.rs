//! Tree-sitter AST (C#) → Complexity IR.
//!
//! Structurally mirrors `java_adapter.rs` (same walk strategy, same heuristics for
//! loop-kind classification, recursion detection, data-dependent-exit detection and
//! allocation detection) but maps onto `tree-sitter-c-sharp`'s grammar, which differs
//! from `tree-sitter-java` in real, confirmed ways (dumped via a throwaway
//! `TreeCursor` walker against representative snippets before writing this, same
//! discipline used for the Java adapter — see tasks.md):
//!
//! - `for_statement` uses field name `initializer` (Java: `init`) for the same slot;
//!   `condition`/`update`/`body` field names match Java.
//! - The loop-counter update expression is `postfix_unary_expression`
//!   (`i++`/`i--`) or `prefix_unary_expression` (`++i`/`--i`) — Java only has
//!   `update_expression`. Neither variant names its `identifier`/operator-token
//!   children (positional, like Java's `update_expression`), so the same
//!   scan-for-identifier-and-token approach from the Java adapter applies to both.
//! - `assignment_expression` has the same `left`/`operator`/`right` fields as Java,
//!   with the same operator set (`+=`/`-=` linear, `*=`/`/=`/`>>=`/`<<=`
//!   logarithmic, and `x = x op k` inspected the same way).
//! - C# has no `enhanced_for_statement`; the equivalent is `foreach_statement`, with
//!   fields `type`/`left`/`right`/`body` (not `enhanced_for_statement`'s Java field
//!   names) — treated as `LoopKind::Linear` for the same reason as Java's for-each
//!   (one iteration per element, no explicit counter).
//! - `if_statement` fields (`condition`/`consequence`/`alternative`) match Java
//!   exactly.
//! - Method calls are `invocation_expression` with a `function` field (Java:
//!   `method_invocation` with a `name` field) that is either a bare `identifier`
//!   (direct call) or a `member_access_expression` with `expression`/`name` fields
//!   (`this.Foo()` — `expression` text checked against `"this"`, same as Java's
//!   `object` field check).
//! - `array_creation_expression` nests the size expression much deeper than Java:
//!   Java exposes a `dimensions` field directly; C# requires descending
//!   `array_creation_expression` → `type` (`array_type`) → `rank`
//!   (`array_rank_specifier`) → named children (the size expressions, if any —
//!   `array_rank_specifier` can be empty when the size comes from an initializer
//!   list instead, e.g. `new int[] { 1, 2, 3 }`, treated as constant since no
//!   input-dependent expression is present).
//! - C# allows **top-level statements** (no enclosing class/method — see
//!   `sandbox/test-snippets-csharp/*/Program.cs`), which `sandbox`'s
//!   `ProcessSandboxRunner#compileCsharp` already treats as an equally valid
//!   program shape (no class-name requirement, unlike Java's `class Main` check).
//!   The grammar represents each top-level statement as a `global_statement` child
//!   of `compilation_unit`. Those are collected into one synthetic method IR named
//!   `"top-level"` so top-level-only files still produce a `MethodComplexity`
//!   entry. A top-level **local function** (`local_function_statement`, C#'s
//!   equivalent of a nested function, commonly used for recursion in top-level
//!   files — same field shape as `method_declaration`: `name`/`parameters`/`body`)
//!   is collected as its own method instead of being folded into `"top-level"`, to
//!   avoid analyzing recursive calls twice.
//!
//! Not covered (deliberately, mirroring the Java adapter's scope): `switch`,
//! labeled loops, lambdas/local functions used as delegates, LINQ, collections
//! (`List<T>` etc. — only `new T[...]` is recognized for space).

use crate::ir::{ComplexityIR, ControlNode, LoopKind, MethodIR};
use tree_sitter::{Node, Parser};

pub fn parse_csharp(source: &str, file_path: &str) -> Result<ComplexityIR, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
        .map_err(|e| format!("falha ao carregar gramática C#: {e}"))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| "tree-sitter falhou ao parsear o arquivo".to_string())?;

    let src = source.as_bytes();
    let root = tree.root_node();
    let mut methods = Vec::new();
    collect_methods(root, src, &mut methods);
    if let Some(top_level) = build_top_level(root, src) {
        methods.push(top_level);
    }

    Ok(ComplexityIR {
        source_file: file_path.to_string(),
        methods,
    })
}

fn collect_methods<'a>(node: Node<'a>, src: &[u8], out: &mut Vec<MethodIR>) {
    if node.kind() == "method_declaration" || node.kind() == "local_function_statement" {
        out.push(build_method(node, src));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_methods(child, src, out);
    }
}

/// Top-level statements (`global_statement` children of `compilation_unit`) that are
/// not a `local_function_statement` (those are collected separately by
/// `collect_methods`) are grouped into one synthetic method so a top-level-only C#
/// file still yields a `MethodComplexity` entry, matching the sandbox's acceptance
/// of top-level-statement C# as a valid program shape.
fn build_top_level(root: Node, src: &[u8]) -> Option<MethodIR> {
    if root.kind() != "compilation_unit" {
        return None;
    }
    let mut cursor = root.walk();
    let statements: Vec<Node> = root
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "global_statement")
        .filter_map(|g| g.named_child(0))
        .filter(|s| s.kind() != "local_function_statement")
        .collect();

    if statements.is_empty() {
        return None;
    }

    let line = statements[0].start_position().row + 1;
    let body = statements
        .into_iter()
        .map(|s| build_statement(s, src, "top-level", false))
        .collect();

    Some(MethodIR {
        name: "top-level".to_string(),
        line,
        params: Vec::new(),
        body,
    })
}

fn build_method(node: Node, src: &[u8]) -> MethodIR {
    let name = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(src).ok())
        .unwrap_or("<anônimo>")
        .to_string();

    let params = node
        .child_by_field_name("parameters")
        .map(|params_node| {
            let mut cursor = params_node.walk();
            params_node
                .children(&mut cursor)
                .filter(|c| c.kind() == "parameter")
                .filter_map(|p| p.child_by_field_name("name"))
                .filter_map(|n| n.utf8_text(src).ok())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    let body_node = node.child_by_field_name("body").filter(|b| b.kind() == "block");
    let mut body = body_node
        .map(|b| build_block(b, src, &name, false))
        .unwrap_or_default();

    // Mirrors the Java adapter's fix: recursion is branching (fibonacci-like)
    // based on how many times the method calls itself across its ENTIRE body,
    // not just within one statement — two separate `Sort(...)` calls in sibling
    // statements (e.g. merge sort) branch just as much as `Fib(n-1) + Fib(n-2)`
    // in a single return. Recount here and overwrite every `Recursion` node's
    // `call_sites` so both shapes are judged by the same method-wide total.
    if let Some(b) = body_node {
        let total_calls = count_self_calls(b, src, &name);
        if total_calls > 0 {
            set_recursion_call_sites(&mut body, total_calls);
        }
    }

    MethodIR {
        name,
        line: node.start_position().row + 1,
        params,
        body,
    }
}

fn set_recursion_call_sites(nodes: &mut [ControlNode], total: usize) {
    for node in nodes.iter_mut() {
        match node {
            ControlNode::Recursion { call_sites, .. } => *call_sites = total,
            ControlNode::Sequential(inner) => set_recursion_call_sites(inner, total),
            ControlNode::Loop { body, .. } => set_recursion_call_sites(body, total),
            ControlNode::Conditional { branches } => {
                for b in branches.iter_mut() {
                    set_recursion_call_sites(b, total);
                }
            }
            ControlNode::DataDependentExit { .. }
            | ControlNode::Allocation { .. }
            | ControlNode::Leaf => {}
        }
    }
}

fn build_block(block: Node, src: &[u8], method_name: &str, in_loop: bool) -> Vec<ControlNode> {
    let mut cursor = block.walk();
    block
        .named_children(&mut cursor)
        .map(|stmt| build_statement(stmt, src, method_name, in_loop))
        .collect()
}

/// The `body` field of a `for`/`while`/`if`/`foreach` can be a `block` or, without
/// braces, a single statement (`for (...) DoIt();`) — normalizes both to `Vec`.
fn build_body_as_vec(node: Node, src: &[u8], method_name: &str, in_loop: bool) -> Vec<ControlNode> {
    if node.kind() == "block" {
        build_block(node, src, method_name, in_loop)
    } else {
        vec![build_statement(node, src, method_name, in_loop)]
    }
}

/// `in_loop` mirrors the Java adapter exactly: indicates whether this statement is
/// (directly or through `if`s) inside a loop body — only in that context does a
/// `break`/`return` become a data-dependent-exit signal for Big-O purposes.
fn build_statement(node: Node, src: &[u8], method_name: &str, in_loop: bool) -> ControlNode {
    let line = node.start_position().row + 1;
    match node.kind() {
        "block" => ControlNode::Sequential(build_block(node, src, method_name, in_loop)),

        "for_statement" => {
            let update = node.child_by_field_name("update");
            let kind = update
                .and_then(|u| classify_update_node(u, src))
                .map(|(k, _)| k)
                .unwrap_or(LoopKind::Unknown);
            let body_node = node.child_by_field_name("body").unwrap();
            ControlNode::Loop {
                kind,
                line,
                body: build_body_as_vec(body_node, src, method_name, true),
            }
        }

        "foreach_statement" => {
            // `foreach (T x in collection)` has no numeric control variable, but
            // iterates exactly once per element — same Linear default as Java's
            // for-each (`enhanced_for_statement`).
            let body_node = node.child_by_field_name("body").unwrap();
            ControlNode::Loop {
                kind: LoopKind::Linear,
                line,
                body: build_body_as_vec(body_node, src, method_name, true),
            }
        }

        "while_statement" => {
            let condition = node.child_by_field_name("condition");
            let body_node = node.child_by_field_name("body").unwrap();
            let is_bsearch = condition
                .map(|c| is_binary_search_idiom(body_node, c, src))
                .unwrap_or(false);
            let kind = if is_bsearch {
                LoopKind::LogarithmicNarrowing
            } else {
                condition
                    .and_then(|c| classify_loop_by_scanning_body(body_node, c, src))
                    .unwrap_or(LoopKind::Unknown)
            };
            ControlNode::Loop {
                kind,
                line,
                // See java_adapter.rs's identical comment: once the two-
                // bound-narrowing idiom is recognized, an early return
                // inside it can only terminate SOONER than the already-
                // proven O(log n) bound, so it's not treated as a
                // DataDependentExit (in_loop=false suppresses that).
                body: build_body_as_vec(body_node, src, method_name, !is_bsearch),
            }
        }

        "do_statement" => {
            let condition = node.child_by_field_name("condition");
            let body_node = node.child_by_field_name("body").unwrap();
            let is_bsearch = condition
                .map(|c| is_binary_search_idiom(body_node, c, src))
                .unwrap_or(false);
            let kind = if is_bsearch {
                LoopKind::LogarithmicNarrowing
            } else {
                condition
                    .and_then(|c| classify_loop_by_scanning_body(body_node, c, src))
                    .unwrap_or(LoopKind::Unknown)
            };
            ControlNode::Loop {
                kind,
                line,
                body: build_body_as_vec(body_node, src, method_name, !is_bsearch),
            }
        }

        "if_statement" => {
            let consequence = node.child_by_field_name("consequence");
            let alternative = node.child_by_field_name("alternative");

            let exits_loop = in_loop
                && (consequence.map(contains_loop_exit).unwrap_or(false)
                    || alternative.map(contains_loop_exit).unwrap_or(false));

            if exits_loop {
                ControlNode::DataDependentExit {
                    line,
                    reason: "break/return condicional dentro de loop — número de \
                             iterações pode depender do valor dos dados de entrada, \
                             não só do tamanho"
                        .to_string(),
                }
            } else {
                let mut branches = Vec::new();
                if let Some(c) = consequence {
                    branches.push(build_body_as_vec(c, src, method_name, in_loop));
                }
                if let Some(a) = alternative {
                    branches.push(build_body_as_vec(a, src, method_name, in_loop));
                }
                ControlNode::Conditional { branches }
            }
        }

        _ => build_leaf_like(node, src, method_name, line),
    }
}

/// `break`/`return` found inside the subtree without descending into a nested
/// loop/switch (that `break` would belong to the inner loop, not ours).
fn contains_loop_exit(node: Node) -> bool {
    match node.kind() {
        "break_statement" | "return_statement" => true,
        "for_statement" | "while_statement" | "do_statement" | "foreach_statement"
        | "switch_expression" | "switch_statement" => false,
        _ => {
            let mut cursor = node.walk();
            let found = node.children(&mut cursor).any(contains_loop_exit);
            found
        }
    }
}

fn build_leaf_like(node: Node, src: &[u8], method_name: &str, line: usize) -> ControlNode {
    let call_sites = count_self_calls(node, src, method_name);
    let allocations = find_allocations(node);

    let mut parts = Vec::new();
    if call_sites > 0 {
        parts.push(ControlNode::Recursion {
            method_name: method_name.to_string(),
            line,
            call_sites,
        });
    }
    parts.extend(allocations);

    match parts.len() {
        0 => ControlNode::Leaf,
        1 => parts.into_iter().next().unwrap(),
        _ => ControlNode::Sequential(parts),
    }
}

fn count_self_calls(node: Node, src: &[u8], method_name: &str) -> usize {
    let mut count = 0;
    if node.kind() == "invocation_expression" {
        if let Some(function) = node.child_by_field_name("function") {
            let is_self_call = match function.kind() {
                "identifier" => function.utf8_text(src).ok() == Some(method_name),
                "member_access_expression" => {
                    let name_matches = function
                        .child_by_field_name("name")
                        .and_then(|n| n.utf8_text(src).ok())
                        == Some(method_name);
                    let object_is_this = function
                        .child_by_field_name("expression")
                        .and_then(|e| e.utf8_text(src).ok())
                        == Some("this");
                    name_matches && object_is_this
                }
                _ => false,
            };
            if is_self_call {
                count += 1;
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        count += count_self_calls(child, src, method_name);
    }
    count
}

fn find_allocations(node: Node) -> Vec<ControlNode> {
    let mut out = Vec::new();
    collect_allocations(node, &mut out);
    out
}

fn collect_allocations(node: Node, out: &mut Vec<ControlNode>) {
    if node.kind() == "array_creation_expression" {
        let size_depends_on_input = array_rank_specifier_of(node)
            .map(dimension_depends_on_input)
            .unwrap_or(false);
        out.push(ControlNode::Allocation {
            line: node.start_position().row + 1,
            size_depends_on_input,
        });
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_allocations(child, out);
    }
}

/// Descends `array_creation_expression` → `type` (`array_type`) → `rank`
/// (`array_rank_specifier`) to reach the node holding the size expression(s) — see
/// module doc comment for why this is deeper than Java's `dimensions` field.
fn array_rank_specifier_of(array_creation: Node) -> Option<Node> {
    array_creation
        .child_by_field_name("type")
        .filter(|t| t.kind() == "array_type")
        .and_then(|t| t.child_by_field_name("rank"))
}

fn dimension_depends_on_input(rank_specifier: Node) -> bool {
    let mut cursor = rank_specifier.walk();
    let depends = rank_specifier
        .named_children(&mut cursor)
        .any(|c| !c.kind().ends_with("_literal"));
    depends
}

/// Classifies a loop-update node — either the `for` statement's `update` field, or
/// (reused) an update statement scanned out of a `while`/`do` body: returns the
/// progression kind and the name of the updated variable, used to cross-reference
/// against the loop condition.
fn classify_update_node(node: Node, src: &[u8]) -> Option<(LoopKind, String)> {
    match node.kind() {
        "postfix_unary_expression" | "prefix_unary_expression" => {
            let ident = find_identifier_text(node, src)?;
            let has_inc_dec = node_text_contains_token(node, src, "++")
                || node_text_contains_token(node, src, "--");
            if has_inc_dec {
                Some((LoopKind::Linear, ident.to_string()))
            } else {
                Some((LoopKind::Unknown, ident.to_string()))
            }
        }
        "assignment_expression" => {
            let left = node.child_by_field_name("left")?.utf8_text(src).ok()?;
            let operator = node.child_by_field_name("operator")?.utf8_text(src).ok()?;
            match operator {
                "+=" | "-=" => Some((LoopKind::Linear, left.to_string())),
                "*=" | "/=" | ">>=" | "<<=" => Some((LoopKind::Logarithmic, left.to_string())),
                "=" => {
                    let right = node.child_by_field_name("right")?;
                    if right.kind() == "binary_expression" {
                        let r_left = right.child_by_field_name("left")?.utf8_text(src).ok()?;
                        let op = right.child_by_field_name("operator")?.utf8_text(src).ok()?;
                        if r_left == left {
                            return match op {
                                "*" | "/" => Some((LoopKind::Logarithmic, left.to_string())),
                                "+" | "-" => Some((LoopKind::Linear, left.to_string())),
                                _ => Some((LoopKind::Unknown, left.to_string())),
                            };
                        }
                    }
                    Some((LoopKind::Unknown, left.to_string()))
                }
                _ => Some((LoopKind::Unknown, left.to_string())),
            }
        }
        _ => None,
    }
}

fn find_identifier_text<'a>(node: Node, src: &'a [u8]) -> Option<&'a str> {
    let mut cursor = node.walk();
    let ident = node
        .children(&mut cursor)
        .find(|c| c.kind() == "identifier")
        .and_then(|n| n.utf8_text(src).ok());
    ident
}

fn node_text_contains_token(node: Node, src: &[u8], token: &str) -> bool {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .any(|c| c.utf8_text(src).ok() == Some(token));
    found
}

/// `while`/`do` have no explicit `update` field — scans the immediate statements of
/// the body (without descending into nested loops, which have their own control
/// variable) looking for an update whose identifier also appears in the condition
/// text. Same heuristic as the Java adapter: the first matching update found is
/// assumed to be the loop's control variable.
fn classify_loop_by_scanning_body(body: Node, condition: Node, src: &[u8]) -> Option<LoopKind> {
    let condition_text = condition.utf8_text(src).ok()?;
    let statements: Vec<Node> = if body.kind() == "block" {
        let mut cursor = body.walk();
        body.named_children(&mut cursor).collect()
    } else {
        vec![body]
    };

    for stmt in statements {
        if is_loop_boundary(stmt.kind()) {
            continue;
        }
        let candidate = match stmt.kind() {
            "expression_statement" => stmt.named_child(0),
            "postfix_unary_expression" | "prefix_unary_expression" | "assignment_expression" => {
                Some(stmt)
            }
            _ => None,
        };
        if let Some(expr) = candidate {
            if let Some((kind, ident)) = classify_update_node(expr, src) {
                if condition_text.contains(&ident) {
                    return Some(kind);
                }
            }
        }
    }
    None
}

fn is_loop_boundary(kind: &str) -> bool {
    matches!(
        kind,
        "for_statement" | "while_statement" | "do_statement" | "foreach_statement"
    )
}

/// Mirrors java_adapter.rs's `is_binary_search_idiom` exactly in algorithm
/// (see its doc comment for the full rationale) — only the grammar-specific
/// helper below (`assignment_in_statement`) differs, for the same reason
/// `classify_update_node` has its own C# copy: `tree-sitter-c-sharp`'s local
/// declaration shape differs from Java's.
fn is_binary_search_idiom(body: Node, condition: Node, src: &[u8]) -> bool {
    let mut bound_idents = Vec::new();
    collect_identifier_names(condition, src, &mut bound_idents);
    bound_idents.dedup();
    if bound_idents.len() < 2 {
        return false;
    }

    let statements: Vec<Node> = if body.kind() == "block" {
        let mut cursor = body.walk();
        body.named_children(&mut cursor).collect()
    } else {
        vec![body]
    };

    let mid_candidates: Vec<String> = statements
        .iter()
        .filter_map(|stmt| assignment_in_statement(*stmt, src))
        .filter(|(_, rhs)| {
            let mut rhs_idents = Vec::new();
            collect_identifier_names(*rhs, src, &mut rhs_idents);
            bound_idents.iter().filter(|b| rhs_idents.contains(b)).count() >= 2
        })
        .map(|(name, _)| name)
        .collect();

    if mid_candidates.is_empty() {
        return false;
    }

    for stmt in &statements {
        if stmt.kind() != "if_statement" {
            continue;
        }
        let consequence = stmt.child_by_field_name("consequence");
        let alternative = stmt.child_by_field_name("alternative");
        let (Some(cons), Some(alt)) = (consequence, alternative) else {
            continue;
        };

        let cons_update = find_mid_derived_bound_update(cons, &bound_idents, &mid_candidates, src);
        let alt_update = find_mid_derived_bound_update(alt, &bound_idents, &mid_candidates, src);

        if let (Some(a), Some(b)) = (cons_update, alt_update) {
            if a != b {
                return true;
            }
        }
    }

    false
}

/// Mirrors java_adapter.rs's function of the same name exactly.
fn find_mid_derived_bound_update(
    branch: Node,
    bound_idents: &[&str],
    mid_candidates: &[String],
    src: &[u8],
) -> Option<String> {
    let statements: Vec<Node> = if branch.kind() == "block" {
        let mut cursor = branch.walk();
        branch.named_children(&mut cursor).collect()
    } else {
        vec![branch]
    };

    for stmt in statements {
        if let Some((lhs, rhs)) = assignment_in_statement(stmt, src) {
            if bound_idents.contains(&lhs.as_str()) {
                let mut rhs_idents = Vec::new();
                collect_identifier_names(rhs, src, &mut rhs_idents);
                if mid_candidates.iter().any(|m| rhs_idents.contains(&m.as_str())) {
                    return Some(lhs);
                }
            }
        }
        if stmt.kind() == "if_statement" {
            if let Some(inner) = stmt.child_by_field_name("consequence") {
                if let Some(found) = find_mid_derived_bound_update(inner, bound_idents, mid_candidates, src) {
                    return Some(found);
                }
            }
            if let Some(inner) = stmt.child_by_field_name("alternative") {
                if let Some(found) = find_mid_derived_bound_update(inner, bound_idents, mid_candidates, src) {
                    return Some(found);
                }
            }
        }
    }

    None
}

/// Extracts `(lhs_name, rhs_node)` from a statement that's either a plain
/// assignment (`x = ...;`) or a local declaration with an initializer
/// (`int x = ...;`). Unlike Java's `local_variable_declaration` (whose
/// `variable_declarator` has an explicit `value` field), C#'s
/// `variable_declarator` has no `value` field at all — confirmed via
/// `tree-sitter-c-sharp`'s own `node-types.json`, not assumed from the
/// grammar being "similar to Java". The initializer is just an unnamed
/// `expression`-typed sibling child alongside the `name` identifier, so it's
/// found by elimination (the one named child that isn't the name node).
fn assignment_in_statement<'a>(stmt: Node<'a>, src: &[u8]) -> Option<(String, Node<'a>)> {
    match stmt.kind() {
        "expression_statement" => {
            let inner = stmt.named_child(0)?;
            if inner.kind() != "assignment_expression" {
                return None;
            }
            let left = inner.child_by_field_name("left")?.utf8_text(src).ok()?.to_string();
            let right = inner.child_by_field_name("right")?;
            Some((left, right))
        }
        "local_declaration_statement" => {
            let mut cursor = stmt.walk();
            for decl_stmt in stmt.named_children(&mut cursor) {
                if decl_stmt.kind() != "variable_declaration" {
                    continue;
                }
                let mut inner_cursor = decl_stmt.walk();
                for declarator in decl_stmt.named_children(&mut inner_cursor) {
                    if declarator.kind() != "variable_declarator" {
                        continue;
                    }
                    let Some(name_node) = declarator.child_by_field_name("name") else {
                        continue;
                    };
                    let Ok(name) = name_node.utf8_text(src) else {
                        continue;
                    };
                    let mut vd_cursor = declarator.walk();
                    let value = declarator
                        .named_children(&mut vd_cursor)
                        .find(|c| c.id() != name_node.id());
                    if let Some(value) = value {
                        return Some((name.to_string(), value));
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Collects the text of every `identifier` node in the subtree.
fn collect_identifier_names<'a>(node: Node, src: &'a [u8], out: &mut Vec<&'a str>) {
    if node.kind() == "identifier" {
        if let Ok(text) = node.utf8_text(src) {
            out.push(text);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_identifier_names(child, src, out);
    }
}
