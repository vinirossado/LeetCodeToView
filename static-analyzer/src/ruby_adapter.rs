//! Tree-sitter AST (Ruby) → Complexity IR.
//!
//! Structurally mirrors `java_adapter.rs`/`csharp_adapter.rs` (same walk strategy,
//! same heuristics for loop-kind classification, recursion detection,
//! data-dependent-exit detection and allocation detection) but maps onto
//! `tree-sitter-ruby`'s grammar, which differs from both Java's and C#'s in real,
//! structural ways — confirmed via a throwaway `TreeCursor` dump tool
//! (`src/bin/dump_ruby_ast.rs`, deleted after this adapter was validated — see
//! tasks.md for the raw dump output) against representative snippets before writing
//! a single line of this file, same discipline used for the other two adapters:
//!
//! - **No C-style `for (init; cond; update)` and no `++`/`--` operators exist in
//!   Ruby at all.** This is the single biggest structural difference from Java/C#.
//!   Ruby's loop update pattern is exclusively `operator_assignment` (`i += 1`,
//!   which — unlike Java's `update_expression`/C#'s `postfix_unary_expression` —
//!   already exposes an explicit `operator` field, no need to scan children for a
//!   `++`/`--` token) or a plain `assignment` (`i = i + 1`) whose right-hand side is
//!   a `binary` node. `classify_update_node` below is consequently simpler than the
//!   other two adapters' — no `find_identifier_text`/`node_text_contains_token`
//!   helpers are needed at all.
//! - Ruby has 4 distinct loop-shaped constructs, all mapped to `ControlNode::Loop`:
//!   `while`/`until` (fields `condition`/`body`, body is a `do` node whose *named*
//!   children ARE the statement list directly — no further "block" wrapper, unlike
//!   Java/C#'s `{ }` block requiring one extra level), `for` (fields
//!   `pattern`/`value`/`body` — `for x in arr` has no numeric control variable,
//!   treated as `LoopKind::Linear` for the same reason as Java's
//!   `enhanced_for_statement`/C#'s `foreach_statement`), and — Ruby's actually
//!   idiomatic style, which Java/C# have no equivalent of — a `call` node with a
//!   `block` field (`arr.each { |x| ... }` / `n.times do |i| ... end`) where the
//!   `method` field's text is one of a deliberately whitelisted set of iteration
//!   methods (see `RECOGNIZED_ITERATION_METHODS` below), also treated as
//!   `LoopKind::Linear` — one iteration per element/count, no explicit counter,
//!   same rationale as `for`.
//! - Ruby has **no separate "expression statement" wrapper node** (Java/C# wrap a
//!   bare expression-as-statement in `expression_statement`) — `assignment`,
//!   `operator_assignment`, `call`, `binary`, etc. appear directly as named children
//!   of whatever container they're in. This actually *simplifies*
//!   `classify_loop_by_scanning_body` relative to the other two adapters (no
//!   `"expression_statement" => stmt.named_child(0)` unwrap step needed).
//! - Every "body-like" grammar slot (`method`'s `body` field, `while`/`until`/`for`'s
//!   `body` field, `if`/`unless`/`elsif`'s `consequence`/`alternative` fields, a
//!   block's `body` field) resolves — after possibly one level of node-kind
//!   difference (`body_statement` for methods and blocks written `do...end`,
//!   `block_body` for blocks written `{ }`, a bare `do` node for while/until/for,
//!   `then`/`else` for if-branches) — to a container node whose **named** children
//!   are the statement list directly (anonymous keyword tokens like the literal
//!   `then`/`else`/`end` text are automatically excluded by `named_children()`).
//!   `container_statements`/`build_container` below replace both `build_block` AND
//!   `build_body_as_vec` from the other two adapters with a single helper, because
//!   Ruby never presents a bare single statement in a body slot without one of these
//!   wrapper kinds (confirmed empirically — even a one-statement `while` body is
//!   still wrapped in a `do` node), unlike Java/C#'s braceless
//!   `for (...) doIt();` shape.
//! - `if`/`unless` share the exact same field shape (`condition`/`consequence`/
//!   `alternative`), and so does `elsif` — Ruby represents an `elsif` chain as
//!   nested `elsif` nodes in the `alternative` slot (structurally identical to how
//!   Java represents `else if` as a nested `if_statement` in `alternative`), so one
//!   shared `build_if_like` handles `if`/`unless`/`elsif` uniformly, recursing on
//!   `alternative` exactly like the other two adapters recurse into a nested
//!   `if_statement`.
//! - Ruby also has a distinct **postfix conditional** idiom with no Java/C#
//!   equivalent — `break if arr[i] == target` / `return 1 if n <= 1` — grammar nodes
//!   `if_modifier`/`unless_modifier`, fields `body` (a single statement, not a list)
//!   /`condition`. This is a common, idiomatic way to write the exact
//!   data-dependent-exit pattern (`break`/`return` guarded by a condition inside a
//!   loop), so it's handled with the same `DataDependentExit` logic as a full
//!   `if`/`break` block, just against a single statement instead of a branch list.
//! - Method calls are `call` nodes with fields `receiver` (optional — absent for a
//!   bare `foo(x)` call, `self` node kind for `self.foo(x)`, anything else for
//!   `obj.foo(x)`), `operator` (the `.` token, when a receiver is present),
//!   `method` (always a plain `identifier` — the called name), `arguments`
//!   (`argument_list`, optional), `block` (optional). Self-recursion is detected the
//!   same way as the other two adapters: `method` field text matches the enclosing
//!   method's name, and `receiver` is either absent or `self`.
//! - Array allocation has two forms, neither of which uses Java/C#'s `_literal`
//!   suffix convention for literal node kinds (`tree-sitter-ruby`'s numeric literal
//!   kinds are bare `integer`/`float`/`rational`, confirmed via this grammar's own
//!   `node-types.json`, not assumed by analogy): an `array` node (`[1, 2, 3]`,
//!   always constant size — it's an element list, not a size parameter, same
//!   treatment as Java's `new int[] {1, 2, 3}`) and a `call` node matching
//!   `Array.new(size)` (`receiver` is a `constant` node with text `"Array"`,
//!   `method` text `"new"`) whose first argument is checked against
//!   `is_constant_size_expr` (an `integer`/`float`/`rational` literal → constant
//!   size; anything else, e.g. `arr.length` or a bare identifier → depends on
//!   input). Only these two forms are recognized — Hash literals/`Hash.new` and
//!   every other collection type are NOT tracked for space, same deliberate scope
//!   limit as Java/C# only tracking `new T[...]`/`array_creation_expression`.
//!
//! Not covered (deliberately, mirroring the other two adapters' own scope limits):
//! `case`/`when` (Ruby's `switch` equivalent), lambdas/`Proc.new`/`->`/blocks used
//! as closures assigned to a variable and invoked indirectly, `method_missing`/
//! `send`/`define_method`/other metaprogramming, endless method definitions
//! (`def square(x) = x * x`, Ruby 3.0+ syntax — a different grammar shape not
//! investigated here), the postfix `while`/`until` modifier applied to a bare
//! statement or to a `begin...end` block (Ruby's do-while-equivalent idiom —
//! `while_modifier`/`until_modifier` nodes exist in the grammar but are rare enough
//! in idiomatic Ruby, and carry enough extra semantic subtlety around the
//! `begin...end` special case, that they were left out rather than guessed at),
//! `loop do ... end` (Kernel#loop, an unbounded loop typically terminated by an
//! internal `break` — not in `RECOGNIZED_ITERATION_METHODS`, so it falls through to
//! the generic leaf handling, same category of gap as Java's un-modeled `switch`: a
//! nested loop/conditional INSIDE an unrecognized construct like this won't get
//! proper structural `Loop`/`Conditional` wrapping, only its recursive calls and
//! allocations still get found by the unconditionally-recursive
//! `count_self_calls`/`collect_allocations` scan), and any call to a non-recursive,
//! non-whitelisted method (no call-graph resolution between methods, same as both
//! other adapters).

use crate::ir::{ComplexityIR, ControlNode, LoopKind, MethodIR};
use tree_sitter::{Node, Parser};

/// Deliberately whitelisted Enumerable/Integer/Range methods that iterate roughly
/// once per element (or once per count, for `times`/`upto`/`downto`/`step`) when
/// called with a block — the same "one iteration per element, no explicit counter"
/// rationale as Java's `enhanced_for_statement`/C#'s `foreach_statement`. This is
/// intentionally NOT "any call with a block is a loop" (that would be structurally
/// wrong for non-iterating block-taking calls like `File.open`, `Thread.new`,
/// `Mutex.synchronize`) — it's a conservative, necessarily incomplete list (Ruby's
/// Enumerable module alone has ~50+ methods) documented here rather than guessed
/// at dynamically.
const RECOGNIZED_ITERATION_METHODS: &[&str] = &[
    "times",
    "upto",
    "downto",
    "step",
    "each",
    "each_index",
    "each_with_index",
    "each_with_object",
    "each_slice",
    "each_cons",
    "each_pair",
    "each_key",
    "each_value",
    "map",
    "map!",
    "collect",
    "collect!",
    "select",
    "select!",
    "filter",
    "filter!",
    "reject",
    "reject!",
    "find",
    "detect",
    "reduce",
    "inject",
    "sort_by",
    "group_by",
    "flat_map",
    "all?",
    "any?",
    "none?",
    "count",
];

fn is_recognized_iteration_method(name: &str) -> bool {
    RECOGNIZED_ITERATION_METHODS.contains(&name)
}

pub fn parse_ruby(source: &str, file_path: &str) -> Result<ComplexityIR, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .map_err(|e| format!("falha ao carregar gramática Ruby: {e}"))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| "tree-sitter falhou ao parsear o arquivo".to_string())?;

    let src = source.as_bytes();
    let mut methods = Vec::new();
    collect_methods(tree.root_node(), src, &mut methods);

    Ok(ComplexityIR {
        source_file: file_path.to_string(),
        methods,
    })
}

fn collect_methods<'a>(node: Node<'a>, src: &[u8], out: &mut Vec<MethodIR>) {
    if node.kind() == "method" {
        out.push(build_method(node, src));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_methods(child, src, out);
    }
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
                .named_children(&mut cursor)
                .filter_map(|p| {
                    // Plain `identifier` params are their own name; every other
                    // parameter kind (`optional_parameter`, `keyword_parameter`,
                    // `splat_parameter`, `block_parameter`, `hash_splat_parameter`)
                    // exposes a `name` field instead (confirmed via node-types.json).
                    // `destructured_parameter`/`forward_parameter` are skipped —
                    // rare enough not to be worth the extra unwrapping here, and
                    // `params` isn't consumed downstream by engine.rs anyway (kept
                    // only for IR completeness/debuggability).
                    if p.kind() == "identifier" {
                        p.utf8_text(src).ok().map(|s| s.to_string())
                    } else {
                        p.child_by_field_name("name")
                            .and_then(|n| n.utf8_text(src).ok())
                            .map(|s| s.to_string())
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let body_node = node.child_by_field_name("body").filter(|b| b.kind() == "body_statement");
    let mut body = body_node
        .map(|b| build_container(b, src, &name, false))
        .unwrap_or_default();

    // Same fix as the other two adapters: recursion is branching (fibonacci-like)
    // based on how many times the method calls itself across its ENTIRE body, not
    // just within one statement — two separate calls in sibling statements branch
    // just as much as `fib(n - 1) + fib(n - 2)` in a single expression. Recount
    // here and overwrite every `Recursion` node's `call_sites` so both shapes are
    // judged by the same method-wide total.
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

/// Every "body-like" container in Ruby's grammar (`body_statement`, `block_body`,
/// the bare `do` node used by `while`/`until`/`for`, and `then`/`else` used by
/// `if`/`unless`) has no fields of its own — its statements are just its *named*
/// children (anonymous keyword tokens like a literal `then`/`else`/`end` are
/// automatically excluded by `named_children()`). One helper replaces both
/// `build_block` and `build_body_as_vec` from the other two adapters — see module
/// doc comment for why Ruby never needs the "single statement without a wrapper"
/// fallback those two adapters have for braceless bodies.
fn container_statements(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn build_container(node: Node, src: &[u8], method_name: &str, in_loop: bool) -> Vec<ControlNode> {
    container_statements(node)
        .into_iter()
        .map(|stmt| build_statement(stmt, src, method_name, in_loop))
        .collect()
}

/// `in_loop` mirrors the other two adapters exactly: indicates whether this
/// statement is (directly or through `if`/`unless`/`elsif`) inside a loop body —
/// only in that context does a `break`/`return` become a data-dependent-exit signal
/// for Big-O purposes.
fn build_statement(node: Node, src: &[u8], method_name: &str, in_loop: bool) -> ControlNode {
    let line = node.start_position().row + 1;
    match node.kind() {
        "while" | "until" => {
            let condition = node.child_by_field_name("condition");
            let Some(body_node) = node.child_by_field_name("body") else {
                return build_leaf_like(node, src, method_name, line);
            };
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
                // Same exception as the other two adapters: once the two-bound
                // narrowing idiom is recognized, an early return inside it can
                // only terminate SOONER than the already-proven O(log n) bound —
                // see ir.rs's doc comment.
                body: build_container(body_node, src, method_name, !is_bsearch),
            }
        }

        "for" => {
            // `for x in collection` — no numeric control variable, but iterates
            // exactly once per element, same Linear default as Java's
            // `enhanced_for_statement`/C#'s `foreach_statement`.
            let Some(body_node) = node.child_by_field_name("body") else {
                return build_leaf_like(node, src, method_name, line);
            };
            ControlNode::Loop {
                kind: LoopKind::Linear,
                line,
                body: build_container(body_node, src, method_name, true),
            }
        }

        "call" => {
            let method_field = node
                .child_by_field_name("method")
                .and_then(|m| m.utf8_text(src).ok());
            let block_field = node.child_by_field_name("block");

            if let (Some(called_name), Some(block_node)) = (method_field, block_field) {
                if is_recognized_iteration_method(called_name) {
                    let body = block_node
                        .child_by_field_name("body")
                        .map(|b| build_container(b, src, method_name, true))
                        .unwrap_or_default();
                    return ControlNode::Loop {
                        kind: LoopKind::Linear,
                        line,
                        body,
                    };
                }
            }
            build_leaf_like(node, src, method_name, line)
        }

        "if" | "unless" | "elsif" => build_if_like(node, src, method_name, in_loop, line),

        "if_modifier" | "unless_modifier" => {
            let body_stmt = node.child_by_field_name("body");
            let exits_loop = in_loop && body_stmt.map(contains_loop_exit).unwrap_or(false);
            if exits_loop {
                ControlNode::DataDependentExit {
                    line,
                    reason: "break/return condicional (postfix if/unless) dentro de loop — \
                             número de iterações pode depender do valor dos dados de \
                             entrada, não só do tamanho"
                        .to_string(),
                }
            } else {
                let branch = body_stmt
                    .map(|b| vec![build_statement(b, src, method_name, in_loop)])
                    .unwrap_or_default();
                ControlNode::Conditional {
                    branches: vec![branch],
                }
            }
        }

        _ => build_leaf_like(node, src, method_name, line),
    }
}

/// Shared by `if`, `unless` and `elsif` — all three have the exact same field
/// shape (`condition`/`consequence`/`alternative`). An `elsif` chain is
/// represented in the grammar as nested `elsif` nodes in the `alternative` slot,
/// structurally identical to how Java represents `else if` as a nested
/// `if_statement` in `alternative` — so recursing into `alternative` here handles
/// the whole chain the same way the other two adapters' nested-if recursion does.
fn build_if_like(node: Node, src: &[u8], method_name: &str, in_loop: bool, line: usize) -> ControlNode {
    let consequence = node.child_by_field_name("consequence");
    let alternative = node.child_by_field_name("alternative");

    let exits_loop = in_loop
        && (consequence.map(contains_loop_exit).unwrap_or(false)
            || alternative.map(contains_loop_exit).unwrap_or(false));

    if exits_loop {
        ControlNode::DataDependentExit {
            line,
            reason: "break/return condicional dentro de loop — número de iterações \
                     pode depender do valor dos dados de entrada, não só do tamanho"
                .to_string(),
        }
    } else {
        let mut branches = Vec::new();
        if let Some(c) = consequence {
            branches.push(build_container(c, src, method_name, in_loop));
        }
        if let Some(a) = alternative {
            if a.kind() == "elsif" {
                let a_line = a.start_position().row + 1;
                branches.push(vec![build_if_like(a, src, method_name, in_loop, a_line)]);
            } else {
                branches.push(build_container(a, src, method_name, in_loop));
            }
        }
        ControlNode::Conditional { branches }
    }
}

/// `break`/`return` found inside the subtree without descending into a nested
/// loop or block (that `break` would belong to the inner iteration, not ours —
/// and a nested `do_block`/`block` introduces its own scope where `break` breaks
/// the enclosing iterator call, not any outer loop). `next` is deliberately NOT
/// treated as an exit signal — unlike `break`, it skips to the next iteration
/// without reducing the loop's iteration count in the worst case, so it carries
/// no Big-O-relevant information here.
fn contains_loop_exit(node: Node) -> bool {
    match node.kind() {
        "break" | "return" => true,
        "while" | "until" | "for" | "do_block" | "block" => false,
        _ => {
            let mut cursor = node.walk();
            let found = node.children(&mut cursor).any(contains_loop_exit);
            found
        }
    }
}

fn build_leaf_like(node: Node, src: &[u8], method_name: &str, line: usize) -> ControlNode {
    let call_sites = count_self_calls(node, src, method_name);
    let allocations = find_allocations(node, src);

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
    if node.kind() == "call" {
        if let Some(method_field) = node.child_by_field_name("method") {
            let name_matches = method_field.utf8_text(src).ok() == Some(method_name);
            let receiver_ok = match node.child_by_field_name("receiver") {
                None => true,
                Some(r) => r.kind() == "self",
            };
            if name_matches && receiver_ok {
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

fn find_allocations(node: Node, src: &[u8]) -> Vec<ControlNode> {
    let mut out = Vec::new();
    collect_allocations(node, src, &mut out);
    out
}

fn collect_allocations(node: Node, src: &[u8], out: &mut Vec<ControlNode>) {
    match node.kind() {
        // Array literal (`[1, 2, 3]`, `[]`) — always a constant size, it's an
        // element list, not a size parameter (same treatment as Java/C#'s array
        // initializer-list form, `new int[] { 1, 2, 3 }`).
        "array" => {
            out.push(ControlNode::Allocation {
                line: node.start_position().row + 1,
                size_depends_on_input: false,
            });
        }
        // `Array.new(size)` — the other allocation shape this adapter recognizes.
        // Hash/other collection constructors are NOT tracked, same deliberate
        // scope limit as Java/C# only tracking `new T[...]`.
        "call" => {
            let is_array_new = node
                .child_by_field_name("receiver")
                .map(|r| r.kind() == "constant" && r.utf8_text(src).ok() == Some("Array"))
                .unwrap_or(false)
                && node.child_by_field_name("method").and_then(|m| m.utf8_text(src).ok()) == Some("new");
            if is_array_new {
                let size_depends_on_input = node
                    .child_by_field_name("arguments")
                    .and_then(|args| args.named_child(0))
                    .map(|first_arg| !is_constant_size_expr(first_arg))
                    .unwrap_or(false); // Array.new() with no args => empty, constant
                out.push(ControlNode::Allocation {
                    line: node.start_position().row + 1,
                    size_depends_on_input,
                });
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_allocations(child, src, out);
    }
}

/// `tree-sitter-ruby` does NOT use Java/C#'s `_literal` suffix convention for
/// numeric literal node kinds (confirmed via this grammar's own
/// `node-types.json`) — its bare kinds are `integer`/`float`/`rational`.
fn is_constant_size_expr(node: Node) -> bool {
    matches!(node.kind(), "integer" | "float" | "rational")
}

/// Classifies a loop-update node found either by `classify_loop_by_scanning_body`
/// (the only path — Ruby has no `for (init; cond; update)`, so there's no
/// dedicated `update` field to read directly the way Java/C#'s `for_statement`
/// has). Much simpler than the other two adapters': Ruby has no `++`/`--`, so
/// there's no `find_identifier_text`/`node_text_contains_token` scanning needed —
/// `operator_assignment` already exposes an explicit `operator` field.
fn classify_update_node(node: Node, src: &[u8]) -> Option<(LoopKind, String)> {
    match node.kind() {
        "operator_assignment" => {
            let left = node.child_by_field_name("left")?.utf8_text(src).ok()?;
            let operator = node.child_by_field_name("operator")?.utf8_text(src).ok()?;
            match operator {
                "+=" | "-=" => Some((LoopKind::Linear, left.to_string())),
                "*=" | "/=" | ">>=" | "<<=" => Some((LoopKind::Logarithmic, left.to_string())),
                _ => Some((LoopKind::Unknown, left.to_string())),
            }
        }
        "assignment" => {
            let left = node.child_by_field_name("left")?.utf8_text(src).ok()?;
            let right = node.child_by_field_name("right")?;
            if right.kind() == "binary" {
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
        _ => None,
    }
}

/// `while`/`until`/`for` have no explicit `update` field — scans the immediate
/// statements of the body (without descending into nested loops, which have
/// their own control variable) looking for an update whose identifier also
/// appears in the condition text. Same heuristic, and same "first match wins"
/// assumption, as the other two adapters. Unlike them, there's no
/// `"expression_statement" => stmt.named_child(0)` unwrap step needed — Ruby has
/// no expression-statement wrapper node, `assignment`/`operator_assignment`
/// appear directly as container children.
fn classify_loop_by_scanning_body(body: Node, condition: Node, src: &[u8]) -> Option<LoopKind> {
    let condition_text = condition.utf8_text(src).ok()?;
    for stmt in container_statements(body) {
        if is_loop_boundary(stmt.kind()) {
            continue;
        }
        let candidate = match stmt.kind() {
            "assignment" | "operator_assignment" => Some(stmt),
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
    matches!(kind, "while" | "until" | "for")
}

/// Mirrors java_adapter.rs's `is_binary_search_idiom` exactly in algorithm (see
/// its doc comment for the full rationale — this does NOT match
/// `classify_update_node`'s `Logarithmic` case at all, since neither bound is
/// ever self-divided; the O(log n) bound instead comes from two DIFFERENT
/// variables converging toward each other via a midpoint). Only the
/// grammar-specific `assignment_in_statement` helper differs.
fn is_binary_search_idiom(body: Node, condition: Node, src: &[u8]) -> bool {
    let mut bound_idents = Vec::new();
    collect_identifier_names(condition, src, &mut bound_idents);
    bound_idents.dedup();
    if bound_idents.len() < 2 {
        return false;
    }

    let statements = container_statements(body);

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
        if stmt.kind() != "if" {
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

/// Mirrors java_adapter.rs's function of the same name. `branch` is expected to
/// be a `then`/`else` container node (a plain `if`'s consequence/alternative) —
/// if it's instead an `elsif` node (an elsif-chained narrowing condition, not
/// seen in practice for this idiom but structurally possible), `container_statements`
/// harmlessly returns nodes that won't match `"assignment"`/`"if"` below, so this
/// just falls through to `None` rather than misinterpreting `elsif`'s own
/// `condition`/`consequence`/`alternative` children as flat statements.
fn find_mid_derived_bound_update(
    branch: Node,
    bound_idents: &[&str],
    mid_candidates: &[String],
    src: &[u8],
) -> Option<String> {
    for stmt in container_statements(branch) {
        if let Some((lhs, rhs)) = assignment_in_statement(stmt, src) {
            if bound_idents.contains(&lhs.as_str()) {
                let mut rhs_idents = Vec::new();
                collect_identifier_names(rhs, src, &mut rhs_idents);
                if mid_candidates.iter().any(|m| rhs_idents.contains(&m.as_str())) {
                    return Some(lhs);
                }
            }
        }
        if stmt.kind() == "if" {
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

/// Extracts `(lhs_name, rhs_node)` from a plain `assignment` node (`x = ...`).
/// Much simpler than the other two adapters' version — Ruby has no separate
/// "local variable declaration with type" grammar shape (no static typing), a
/// `mid = ...` midpoint computation is just a regular `assignment`, the same node
/// kind as any other variable assignment.
fn assignment_in_statement<'a>(stmt: Node<'a>, src: &[u8]) -> Option<(String, Node<'a>)> {
    if stmt.kind() != "assignment" {
        return None;
    }
    let left = stmt.child_by_field_name("left")?.utf8_text(src).ok()?.to_string();
    let right = stmt.child_by_field_name("right")?;
    Some((left, right))
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
