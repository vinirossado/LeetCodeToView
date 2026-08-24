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

    let body = node
        .child_by_field_name("body")
        .filter(|b| b.kind() == "block")
        .map(|b| build_block(b, src, &name, false))
        .unwrap_or_default();

    MethodIR {
        name,
        line: node.start_position().row + 1,
        params,
        body,
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
            let kind = condition
                .and_then(|c| classify_loop_by_scanning_body(body_node, c, src))
                .unwrap_or(LoopKind::Unknown);
            ControlNode::Loop {
                kind,
                line,
                body: build_body_as_vec(body_node, src, method_name, true),
            }
        }

        "do_statement" => {
            let condition = node.child_by_field_name("condition");
            let body_node = node.child_by_field_name("body").unwrap();
            let kind = condition
                .and_then(|c| classify_loop_by_scanning_body(body_node, c, src))
                .unwrap_or(LoopKind::Unknown);
            ControlNode::Loop {
                kind,
                line,
                body: build_body_as_vec(body_node, src, method_name, true),
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
