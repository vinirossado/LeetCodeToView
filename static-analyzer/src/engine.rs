//! Complexity IR → estimativa de Big-O.
//!
//! Heurística deliberadamente simples (ver limitações no `spec.md`/`ir.rs`):
//! - Profundidade de aninhamento de loops lineares = grau do polinômio
//!   (1 loop = O(n), 2 loops aninhados = O(n²), ...).
//! - Loop cujo contador é dividido/multiplicado por uma constante a cada iteração
//!   = O(log n); aninhado dentro de um loop linear (ou vice-versa) = O(n log n).
//! - Recursão com exatamente 1 ponto de chamada direto (ex: fatorial, contagem
//!   regressiva) é tratada como O(n) — aproximação por "1 chamada = 1 nível a
//!   menos", não resolve a relação de recorrência de verdade. Recursão com 2+
//!   pontos de chamada (ex: fibonacci ingênuo) não é modelada com confiança:
//!   cai em "não foi possível determinar" em vez de cravar O(2^n) sem prova.
//! - Qualquer `DataDependentExit` (break/return condicional dentro de loop) torna
//!   o resultado "não foi possível determinar" — mesmo em casos onde um humano
//!   provaria o pior caso (ex: busca linear ainda é O(n) mesmo com early exit).
//!   Preferimos essa imprecisão a uma resposta com confiança falsa.

use crate::ir::{ComplexityIR, ControlNode, LoopKind, MethodIR};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum TimeComplexity {
    Constant,
    Logarithmic,
    Linear,
    Linearithmic,
    Polynomial(u32),
    Unknown(String),
}

impl std::fmt::Display for TimeComplexity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimeComplexity::Constant => write!(f, "O(1)"),
            TimeComplexity::Logarithmic => write!(f, "O(log n)"),
            TimeComplexity::Linear => write!(f, "O(n)"),
            TimeComplexity::Linearithmic => write!(f, "O(n log n)"),
            TimeComplexity::Polynomial(k) => write!(f, "O(n^{k})"),
            TimeComplexity::Unknown(reason) => write!(f, "não foi possível determinar ({reason})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum SpaceComplexity {
    Constant,
    Linear,
    Unknown(String),
}

impl std::fmt::Display for SpaceComplexity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpaceComplexity::Constant => write!(f, "O(1)"),
            SpaceComplexity::Linear => write!(f, "O(n)"),
            SpaceComplexity::Unknown(reason) => {
                write!(f, "não foi possível determinar ({reason})")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MethodComplexity {
    pub method_name: String,
    pub line: usize,
    pub time: TimeComplexity,
    pub space: SpaceComplexity,
    pub evidence: Vec<String>,
}

pub fn analyze(ir: &ComplexityIR) -> Vec<MethodComplexity> {
    ir.methods.iter().map(analyze_method).collect()
}

fn analyze_method(method: &MethodIR) -> MethodComplexity {
    let mut evidence = Vec::new();
    let time_shape = classify_seq(&method.body, &mut evidence);
    let space_shape = classify_space(&method.body, &mut evidence);

    MethodComplexity {
        method_name: method.name.clone(),
        line: method.line,
        time: time_shape.into(),
        space: space_shape.into(),
        evidence,
    }
}

/// Representação interna do "formato" de crescimento, antes de virar `TimeComplexity`.
/// Existe separada de `TimeComplexity` porque a composição (loop dentro de loop,
/// max entre branches) precisa de uma ordem total simples entre os casos que a
/// heurística cobre — não é uma álgebra geral de Big-O.
#[derive(Debug, Clone, PartialEq)]
enum Shape {
    Const,
    Log,
    Poly(u32), // n^k, k >= 1
    NLogN,
    Unknown(String),
}

impl From<Shape> for TimeComplexity {
    fn from(s: Shape) -> Self {
        match s {
            Shape::Const => TimeComplexity::Constant,
            Shape::Log => TimeComplexity::Logarithmic,
            Shape::Poly(1) => TimeComplexity::Linear,
            Shape::Poly(k) => TimeComplexity::Polynomial(k),
            Shape::NLogN => TimeComplexity::Linearithmic,
            Shape::Unknown(r) => TimeComplexity::Unknown(r),
        }
    }
}

fn shape_rank(s: &Shape) -> u32 {
    match s {
        Shape::Const => 0,
        Shape::Log => 10,
        Shape::Poly(k) if *k == 1 => 20,
        Shape::NLogN => 25,
        Shape::Poly(k) => 20 * k,
        Shape::Unknown(_) => u32::MAX,
    }
}

fn max_shape(a: Shape, b: Shape) -> Shape {
    match (&a, &b) {
        (Shape::Unknown(ra), Shape::Unknown(rb)) => {
            if ra == rb {
                a
            } else {
                Shape::Unknown(format!("{ra}; {rb}"))
            }
        }
        (Shape::Unknown(_), _) => a,
        (_, Shape::Unknown(_)) => b,
        _ => {
            if shape_rank(&a) >= shape_rank(&b) {
                a
            } else {
                b
            }
        }
    }
}

fn classify_seq(nodes: &[ControlNode], evidence: &mut Vec<String>) -> Shape {
    nodes
        .iter()
        .map(|n| classify_node(n, evidence))
        .fold(Shape::Const, max_shape)
}

fn classify_node(node: &ControlNode, evidence: &mut Vec<String>) -> Shape {
    match node {
        ControlNode::Leaf => Shape::Const,
        ControlNode::Allocation { .. } => Shape::Const, // custo de tempo da alocação em si é O(1)/O(n) amortizado; espaço é tratado à parte
        ControlNode::Sequential(inner) => classify_seq(inner, evidence),
        ControlNode::Conditional { branches } => branches
            .iter()
            .map(|b| classify_seq(b, evidence))
            .fold(Shape::Const, max_shape),
        ControlNode::DataDependentExit { line, reason } => {
            evidence.push(format!(
                "linha {line}: saída condicional (break/return) dentro de loop — {reason}"
            ));
            Shape::Unknown(format!("saída condicional na linha {line}"))
        }
        ControlNode::Recursion {
            method_name,
            line,
            call_sites,
        } => {
            if *call_sites == 1 {
                evidence.push(format!(
                    "linha {line}: recursão direta de '{method_name}' (1 ponto de chamada) tratada como O(n)"
                ));
                Shape::Poly(1)
            } else {
                evidence.push(format!(
                    "linha {line}: recursão direta de '{method_name}' com {call_sites} pontos de chamada — possível ramificação (ex: fibonacci ingênuo), engine não resolve relação de recorrência"
                ));
                Shape::Unknown(format!(
                    "recursão com {call_sites} pontos de chamada na linha {line}"
                ))
            }
        }
        ControlNode::Loop { kind, line, body } => {
            let inner = classify_seq(body, evidence);
            match kind {
                LoopKind::Unknown => {
                    evidence.push(format!(
                        "linha {line}: loop com padrão de incremento não reconhecido pela heurística"
                    ));
                    Shape::Unknown(format!("padrão de incremento não reconhecido na linha {line}"))
                }
                LoopKind::Linear => {
                    evidence.push(format!("linha {line}: loop com incremento linear"));
                    compose_linear(inner, *line)
                }
                LoopKind::Logarithmic => {
                    evidence.push(format!(
                        "linha {line}: loop com incremento logarítmico (divisão/multiplicação por constante)"
                    ));
                    compose_log(inner, *line)
                }
            }
        }
    }
}

fn compose_linear(inner: Shape, line: usize) -> Shape {
    match inner {
        Shape::Const => Shape::Poly(1),
        Shape::Poly(k) => Shape::Poly(k + 1),
        Shape::Log => Shape::NLogN,
        Shape::NLogN => Shape::Unknown(format!(
            "loop linear (linha {line}) contendo um corpo já O(n log n) — combinação não coberta pela heurística atual"
        )),
        Shape::Unknown(r) => Shape::Unknown(r),
    }
}

fn compose_log(inner: Shape, line: usize) -> Shape {
    match inner {
        Shape::Const => Shape::Log,
        Shape::Poly(1) => Shape::NLogN,
        Shape::Poly(k) => Shape::Unknown(format!(
            "loop logarítmico (linha {line}) contendo corpo O(n^{k}) — produto não coberto pela heurística atual"
        )),
        Shape::Log => Shape::Unknown(format!(
            "loops logarítmicos aninhados (linha {line}, O(log² n)) não cobertos pela heurística atual"
        )),
        Shape::NLogN => Shape::Unknown(format!(
            "loop logarítmico (linha {line}) contendo corpo já O(n log n) — combinação não coberta pela heurística atual"
        )),
        Shape::Unknown(r) => Shape::Unknown(r),
    }
}

/// Espaço: bem mais simples que tempo por design (ver spec.md — "talvez tamanho de
/// estruturas alocadas", não precisa ser sofisticado). Considera:
/// - qualquer alocação de array cuja dimensão não é uma constante literal => O(n);
/// - qualquer recursão direta => pilha de chamada O(n) (1 ponto de chamada) ou
///   "não determinado" (2+ pontos de chamada, mesmo racional do tempo);
/// - do contrário, O(1).
/// Não soma allocations (ex: 2 arrays de tamanho n não viram O(n²) de espaço) —
/// simplificação deliberada.
fn classify_space(nodes: &[ControlNode], evidence: &mut Vec<String>) -> SpaceShapeInternal {
    let mut result = SpaceShapeInternal::Const;
    scan_space(nodes, &mut result, evidence);
    result
}

#[derive(Debug, Clone, PartialEq)]
enum SpaceShapeInternal {
    Const,
    Linear,
    Unknown(String),
}

impl From<SpaceShapeInternal> for SpaceComplexity {
    fn from(s: SpaceShapeInternal) -> Self {
        match s {
            SpaceShapeInternal::Const => SpaceComplexity::Constant,
            SpaceShapeInternal::Linear => SpaceComplexity::Linear,
            SpaceShapeInternal::Unknown(r) => SpaceComplexity::Unknown(r),
        }
    }
}

fn bump_space(result: &mut SpaceShapeInternal, candidate: SpaceShapeInternal) {
    let rank = |s: &SpaceShapeInternal| match s {
        SpaceShapeInternal::Const => 0,
        SpaceShapeInternal::Linear => 1,
        SpaceShapeInternal::Unknown(_) => 2,
    };
    if rank(&candidate) > rank(result) {
        *result = candidate;
    }
}

fn scan_space(nodes: &[ControlNode], result: &mut SpaceShapeInternal, evidence: &mut Vec<String>) {
    for node in nodes {
        match node {
            ControlNode::Allocation {
                line,
                size_depends_on_input,
            } => {
                if *size_depends_on_input {
                    evidence.push(format!(
                        "linha {line}: alocação com tamanho dependente de entrada — espaço O(n)"
                    ));
                    bump_space(result, SpaceShapeInternal::Linear);
                }
            }
            ControlNode::Recursion {
                method_name,
                line,
                call_sites,
            } => {
                if *call_sites == 1 {
                    evidence.push(format!(
                        "linha {line}: recursão de '{method_name}' — pilha de chamadas O(n)"
                    ));
                    bump_space(result, SpaceShapeInternal::Linear);
                } else {
                    bump_space(
                        result,
                        SpaceShapeInternal::Unknown(format!(
                            "recursão com {call_sites} pontos de chamada na linha {line} — profundidade de pilha não modelada"
                        )),
                    );
                }
            }
            ControlNode::Loop { body, .. } => scan_space(body, result, evidence),
            ControlNode::Sequential(inner) => scan_space(inner, result, evidence),
            ControlNode::Conditional { branches } => {
                for b in branches {
                    scan_space(b, result, evidence);
                }
            }
            ControlNode::DataDependentExit { .. } | ControlNode::Leaf => {}
        }
    }
}
