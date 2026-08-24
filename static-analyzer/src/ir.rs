//! Complexity IR: representação própria e independente de linguagem do que importa
//! para estimar complexidade — estrutura de loops (aninhamento, padrão de incremento),
//! recursão, branches e alocações. Não modela semântica completa da linguagem (não é
//! um AST genérico) — só o subconjunto de estrutura de controle relevante para o
//! Complexity Engine.
//!
//! Limitações conhecidas (documentadas, não acidentais):
//! - Não resolve valores em tempo de execução: um loop `for (i=0; i<n; i++)` e um
//!   `for (i=0; i<1000000; i++)` são tratados igual (ambos "linear em função do
//!   tamanho de alguma entrada"), porque a IR não faz avaliação de constantes.
//! - Chamadas de método que não sejam recursão direta (mesmo nome, dentro do próprio
//!   corpo) não são seguidas — não há resolução de call graph entre métodos.
//! - `DataDependentExit` é uma heurística conservadora: qualquer `break`/`return`
//!   dentro de um `if` aninhado em um loop é tratado como potencial saída antecipada
//!   dependente de dado, mesmo quando um humano conseguiria provar que o pior caso
//!   ainda é O(n) (busca linear) ou O(log n) (busca binária). Preferimos essa
//!   imprecisão (falso "não determinado") a cravar um Big-O que pode estar errado.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LoopKind {
    /// Variável de controle andando por passo constante (i++, i--, i += k, i -= k).
    Linear,
    /// Variável de controle multiplicada/dividida por uma constante a cada iteração
    /// (i /= 2, i *= 2, i >>= 1, i = i / k, i = i * k).
    Logarithmic,
    /// Update presente, mas não reconhecido pelas heurísticas acima (ex: incremento
    /// não-constante, chamada de método no update, múltiplas variáveis).
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub enum ControlNode {
    /// Sequência de nós irmãos (ex: statements dentro de um mesmo bloco).
    Sequential(Vec<ControlNode>),

    Loop {
        kind: LoopKind,
        line: usize,
        body: Vec<ControlNode>,
    },

    /// Bifurcação (if/else, switch). Cada branch é avaliado separadamente pelo engine
    /// (pior caso = max entre branches) — não modelamos custo condicional combinado.
    Conditional { branches: Vec<Vec<ControlNode>> },

    /// Chamada recursiva direta (método chamando a si mesmo pelo nome, achado dentro
    /// do próprio corpo do método). `call_sites` conta quantos pontos de chamada
    /// distintos foram encontrados (1 = recursão "linear"-like; >1 = potencial
    /// recursão com ramificação, ex: fibonacci ingênuo).
    Recursion {
        method_name: String,
        line: usize,
        call_sites: usize,
    },

    /// Ponto de saída (break/return) encontrado dentro de um `if` aninhado em um
    /// loop — sinal de que o número de iterações pode depender do valor dos dados
    /// de entrada, não só do seu tamanho.
    DataDependentExit { line: usize, reason: String },

    /// Alocação de estrutura de dados cujo tamanho não é uma constante literal
    /// (ex: `new int[n]`, `new int[arr.length]`) — usado pela estimativa de espaço.
    Allocation { line: usize, size_depends_on_input: bool },

    /// Statement sem impacto estrutural na complexidade (atribuição simples, chamada
    /// de método não-recursiva, print, etc.).
    Leaf,
}

#[derive(Debug, Clone, Serialize)]
pub struct MethodIR {
    pub name: String,
    pub line: usize,
    pub params: Vec<String>,
    pub body: Vec<ControlNode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComplexityIR {
    pub source_file: String,
    pub methods: Vec<MethodIR>,
}
