# Tasks

Backlog aberto apenas — itens já concluídos foram removidos deste arquivo
(o histórico completo de decisões/achados/validações de cada um continua no
`git log`/diffs, já que um processo externo faz auto-commit contínuo em
`main`). Cada item abaixo preserva o contexto/raciocínio original, não só o
título, porque é o que uma futura sessão precisa pra retomar o trabalho.

## Fase 0 — Setup

- [ ] **Definir convenções de commit/branch — não resolvido, achado real ao investigar, não deliberadamente ignorado.** `git log`/`git branch -a` mostram a realidade atual: um processo externo de auto-commit (não esta sessão, não nenhuma sessão Claude — já observado e sinalizado ao usuário antes) commita direto em `main` continuamente, sem branches de feature nem PRs — `git branch -a` só lista `main` (+ worktrees temporários desta sessão). O estilo de mensagem se aproxima de Conventional Commits (`feat: ...`, `feat(escopo): ...`) na maioria, mas sem ferramenta enforçando isso (achados fora do padrão existem: `af9776e front`, `4bb7ad3 prepare build`) e sem `CONTRIBUTING.md`/hook de lint de commit configurado. Deixado em aberto de propósito: "definir uma convenção" que ninguém (nem processo automatizado, nem humano) vai de fato seguir/enforçar seria documentação decorativa, não uma correção real — e não é algo que uma sessão de agente deva decidir sozinha (convenção de branch/commit é escolha de processo de time, não técnica). Registrado aqui pra quando o dono do projeto quiser decidir isso de verdade (e possivelmente configurar um hook/CI check que realmente enforce, não só documente).

## Fase 0.5 — Spike do Sandbox (sem API, sem frontend)

- [ ] Capabilities mínimas pra rodar o `nsjail` **em si** sem `--privileged`/root completo (o próprio processo supervisor, não o código sandboxado que ele lança): testado `cap_sys_admin,cap_sys_chroot,cap_net_admin,cap_sys_resource,cap_setuid,cap_setgid` via `setcap` num Linux real — ainda insuficiente (falha em `setgroups` do user namespace). Rodar como root (ou `--privileged` equivalente) funciona; achar o conjunto mínimo exato de capabilities fica pra Fase 2 (não é bloqueante pro spike, só relevante pra hardening). **Não confundir com "usuário não-root DENTRO do jail"** (o código sandboxado, não o nsjail supervisor) — esse é um problema diferente e menor, já resolvido separadamente (ver `sandbox/src/java.rs`/`csharp.rs`/`ruby.rs`, `--uid_mapping`/`--gid_mapping`).
- [ ] Mitigação estrutural pra entradas *muito* grandes fica como ideia futura (não bloqueante): trocar JDI/JDWP por agente de bytecode (`java.lang.instrument` + ASM/ByteBuddy) sem round-trip de protocolo — não entra no MVP, só se o cap de escopo (step/output limits já implementados) se mostrar insuficiente na prática.

## Fase 1 — MVP com Java + C#

- [ ] **Item aberto derivado de um achado anterior**: investigar por que o stepper do ICorDebug às vezes trava indefinidamente dentro de `StartCore` (implementação nativa de `Thread.Start()`) — travamento pré-existente, visível ao implementar o bloqueio multi-thread. Resolver isso tornaria a detecção de multi-thread em C# tão confiável quanto a de Java, e possivelmente destravaria stepping através de qualquer código C# que chame `Thread.Start()`/APIs relacionadas de forma mais ampla (não só o caso multi-thread bloqueado). **Confirmado que o Just My Code (JMC) implementado posteriormente NÃO resolve isso** — testado `MultiThreadCs` (8 threads) contra a stack com JMC, bateu no mesmo travamento pré-existente (`status: failed`, evento `timeout`); JMC muda ONDE o stepper para, não resolve a transição interna do CLR que trava o mecanismo de step em si. O mesmo tipo de travamento (stepper síncrono de granularidade fina interagindo mal com certas transições internas do runtime — nativo↔gerenciado num caso, unwind de exceção não capturada em outro) também aparece ao investigar por que uma exceção C# não tratada não produz um erro específico (cai em timeout genérico) — ver `sandbox/src/com.rs`'s `cb_exception` pro estado atual e as duas hipóteses já testadas e descartadas (`Deactivate()` do stepper ativo, verificação de `ExitProcess`).
- [ ] Definir contrato/IPC entre API (Quarkus) e Sandbox Controller (Rust) de verdade — o que existe hoje (`ProcessSandboxRunner`) é a escolha mais simples possível (fork+exec direto do binário `sandbox-runner`, sem fila/gRPC/socket), não uma decisão definitiva validada; falta decidir se isso basta pra produção ou se precisa de fila (Redis/NATS) pra desacoplar API de sandbox controller rodando em hosts diferentes.

## Fase 2 — Segurança / Hardening

- [ ] Avaliar migração de nsjail para microVMs (Firecracker) para cargas mais sensíveis.
- [ ] Scheduling/bin-packing de execuções a nível de cluster (cgroup limita 1 execução, mas não impede "noisy neighbor" entre várias execuções simultâneas disputando CPU real no mesmo host).

## Fase 4 — Produto

- [ ] Persistência de execuções (histórico por usuário).
- [ ] Autenticação/contas.
- [ ] Documentação pública da API.
