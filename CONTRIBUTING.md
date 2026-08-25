# Contribuindo

## Mensagens de commit

Este projeto segue [Conventional Commits](https://www.conventionalcommits.org/) — é o padrão de fato já usado na maior parte do histórico (`feat: ...`, `feat(escopo): ...`, `fix: ...`), então formalizar isso é só dar nome ao que já acontece na prática, não introduzir algo novo.

Tipos usados neste repo: `feat` (funcionalidade nova), `fix` (correção de bug), `refactor` (mudança de estrutura sem mudar comportamento), `docs` (só documentação), `test` (só testes), `chore` (build/config/tooling).

Escopo opcional entre parênteses quando ajuda a localizar a mudança (`feat(sandbox): ...`, `fix(frontend): ...`), livre — sem uma lista fixa de escopos válidos.

## Sem enforcement automatizado — decisão deliberada, não lacuna

Este repositório é atualizado por um processo externo de auto-commit direto em `main` (sem branches de feature, sem PRs, sem branch protection) — checar `git branch -a`/`git log` confirma isso, é a realidade operacional atual, não uma hipótese. Um hook `commit-msg` bloqueante quebraria esse processo em vez de guiá-lo, a menos que o próprio processo seja atualizado para respeitar o hook — o que está fora do escopo desta decisão.

Por isso esta convenção é **documentada, não enforçada por ferramenta** — `CONTRIBUTING.md` describes o padrão para quem lê/escreve commits manualmente; nenhum hook de lint de commit ou gate de CI bloqueia uma mensagem fora do padrão. Se o processo de auto-commit for substituído por um fluxo baseado em PR no futuro, aí sim vale reconsiderar `commitlint`/hooks reais — sem esse pré-requisito, adicionar a ferramenta seria documentação decorativa se fingindo de enforcement real.
