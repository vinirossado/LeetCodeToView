//! Tree-sitter AST (Java) → Complexity IR.
//!
//! Cobre: `for`/`while`/`do`/for-each, classificação de padrão de incremento
//! (linear vs. logarítmico) via inspeção textual do `update` (ou, no caso de
//! `while`/`do`, varrendo o corpo por uma atualização da variável referenciada na
//! condição), recursão direta (chamada ao próprio método pelo nome), detecção de
//! `break`/`return` condicional dentro de loop (heurística de saída dependente de
//! dado) e alocação de array cujo tamanho não é uma constante literal.
//!
//! Não cobre (limitações conhecidas, deliberadamente fora de escopo desta 1ª
//! versão): `switch`, loops rotulados (`label: for`), lambdas/classes anônimas,
//! chamadas de método não-recursivas (sem resolução de call graph entre métodos),
//! coleções (`ArrayList`, `HashMap`) — só `new T[...]` é reconhecido para espaço.

use crate::ir::{ComplexityIR, ControlNode, LoopKind, MethodIR};
use tree_sitter::{Node, Parser};

pub fn parse_java(source: &str, file_path: &str) -> Result<ComplexityIR, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .map_err(|e| format!("falha ao carregar gramática Java: {e}"))?;

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
    if node.kind() == "method_declaration" {
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
                .children(&mut cursor)
                .filter(|c| c.kind() == "formal_parameter" || c.kind() == "spread_parameter")
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

    // Recursion is branching (fibonacci-like) based on how many times the method
    // calls itself across its ENTIRE body, not just within one statement — two
    // separate `sort(...)` calls in sibling statements (e.g. merge sort) branch
    // just as much as `fib(n-1) + fib(n-2)` in a single return. Recount here and
    // overwrite every `Recursion` node's `call_sites` so both shapes are judged
    // by the same method-wide total.
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

/// O campo `body` de um `for`/`while`/`if` pode ser um `block` ou, sem chaves, um
/// único statement (`for (...) doIt();`) — normaliza os dois casos para `Vec`.
fn build_body_as_vec(node: Node, src: &[u8], method_name: &str, in_loop: bool) -> Vec<ControlNode> {
    if node.kind() == "block" {
        build_block(node, src, method_name, in_loop)
    } else {
        vec![build_statement(node, src, method_name, in_loop)]
    }
}

/// `in_loop` indica se este statement está (direta ou indiretamente, através de
/// `if`s) dentro do corpo de um loop — só nesse contexto um `break`/`return`
/// condicional é uma saída antecipada que afeta a contagem de iterações. Um
/// `if/return` fora de qualquer loop (ex: caso-base de recursão, guard clause) é
/// controle de fluxo normal, não uma "saída dependente de dado" no sentido de Big-O.
fn build_statement(node: Node, src: &[u8], method_name: &str, in_loop: bool) -> ControlNode {
    let line = node.start_position().row + 1;
    match node.kind() {
        "block" => ControlNode::Sequential(build_block(node, src, method_name, in_loop)),

        "for_statement" => {
            let update = node
                .children_by_field_name("update", &mut node.walk())
                .next();
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

        "enhanced_for_statement" => {
            // for-each (`for (T x : collection)`) não expõe uma variável de controle
            // numérica — mas itera exatamente uma vez por elemento da coleção, o que
            // é o próprio padrão do caso Linear (percorrer uma estrutura de tamanho
            // proporcional à entrada).
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

/// `break`/`return` encontrado dentro do subtree, sem descer para dentro de um
/// loop/switch aninhado (esse `break` pertenceria ao loop interno, não ao nosso).
fn contains_loop_exit(node: Node) -> bool {
    match node.kind() {
        "break_statement" | "return_statement" => true,
        "for_statement" | "while_statement" | "do_statement" | "enhanced_for_statement"
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
    if node.kind() == "method_invocation" {
        let name_matches = node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(src).ok())
            == Some(method_name);
        let object_ok = match node.child_by_field_name("object") {
            None => true,
            Some(obj) => obj.utf8_text(src).ok() == Some("this"),
        };
        if name_matches && object_ok {
            count += 1;
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
    if node.kind() == "array_creation_expression" {
        let size_depends_on_input = node
            .child_by_field_name("dimensions")
            .map(|dims| dimension_depends_on_input(dims, src))
            .unwrap_or(false);
        out.push(ControlNode::Allocation {
            line: node.start_position().row + 1,
            size_depends_on_input,
        });
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_allocations(child, src, out);
    }
}

fn dimension_depends_on_input(dimensions_expr: Node, _src: &[u8]) -> bool {
    let mut cursor = dimensions_expr.walk();
    let depends = dimensions_expr
        .named_children(&mut cursor)
        .any(|c| !c.kind().ends_with("_literal"));
    depends
}

/// Classifica um nó de `update` de `for` (ou, reaproveitado, um statement de
/// atualização dentro de um `while`/`do`): retorna o tipo de progressão e o nome
/// da variável atualizada, usado para cruzar com a condição do loop.
fn classify_update_node(node: Node, src: &[u8]) -> Option<(LoopKind, String)> {
    match node.kind() {
        "update_expression" => {
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

/// `while`/`do` não têm campo `update` explícito — varre os statements imediatos do
/// corpo (sem entrar em loops aninhados, que têm sua própria variável de controle)
/// procurando uma atualização cujo identificador também apareça no texto da
/// condição. Heurística: a primeira atualização encontrada que "bate" com a
/// condição é assumida como a variável de controle do loop.
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
            "update_expression" | "assignment_expression" => Some(stmt),
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
        "for_statement" | "while_statement" | "do_statement" | "enhanced_for_statement"
    )
}
