# Especificação técnica

## Arquitetura

```
                    WEB EDITOR
                        │
              código + linguagem
                        │
                        ▼
                 API / Quarkus
                        │
              ┌─────────┴─────────┐
              │                   │
              ▼                   ▼
       STATIC ANALYZER       SANDBOX RUNNER
              │                   │
         AST / IR             execução isolada
              │                   │
              ▼                   ▼
        Complexity Engine     Execution Events
              │                   │
              └─────────┬─────────┘
                        ▼
                  WebSocket/SSE
                        │
                        ▼
                  VISUALIZADOR
```

## Stack inicial

| Componente | Tecnologia |
|---|---|
| Frontend | TypeScript + Angular |
| API | Java + Quarkus |
| Sandbox Controller | Rust |
| Primeiro runtime | Java + C# (juntos) |
| Parsing | Tree-sitter |
| Complexidade | Complexity IR própria |
| Comunicação | WebSocket |
| Isolamento | nsjail (namespaces + seccomp-bpf) |
| Evolução | Firecracker/microVM |

## API

### Modelo de execução: trace-and-replay

O código roda do início ao fim dentro da sandbox (até completar ou até estourar um limite de segurança), gerando um trace completo de eventos. A sandbox é destruída logo depois — não existe processo pausado esperando comando do usuário. "Andar linha por linha", breakpoints e inspeção de variáveis em qualquer ponto são navegação **client-side** sobre o trace já recebido, sem round-trip pro servidor.

Os eventos ainda são transmitidos via WebSocket durante a execução (dá a sensação de tempo real enquanto roda), mas o trace completo também fica disponível via REST para quem carregar a página depois da execução terminar, ou reconectar no meio.

Esse modelo elimina a necessidade de canal de controle bidirecional (step/continue vindo do cliente) e de timeout de sessão — o tempo de vida da sandbox é sempre curto e previsível, do início ao fim de uma execução.

### Criar execução

`POST /executions`

```json
{
  "language": "java",
  "code": "int x = 10;\nfor (int i = 0; i < x; i++) {\n    System.out.println(i);\n}"
}
```

Retorna:

```json
{
  "execution_id": "b3f1c2a4-6e9d-4a2b-9f3e-1d7c8a0b5f6e"
}
```

`execution_id` é um UUID v4, gerado pela API a cada execução — não sequencial, não previsível.

### Eventos de execução

`GET /executions/b3f1c2a4-6e9d-4a2b-9f3e-1d7c8a0b5f6e/events`

Preferencialmente via WebSocket para atualização em tempo real.

Exemplo:

```json
{
  "type": "step",
  "line": 2,
  "locals": {
    "x": 10,
    "i": 4
  },
  "stack": ["main"],
  "time_ns": 18320,
  "memory_bytes": 12480
}
```

Outros tipos de evento: `timeout`, `memory_limit_exceeded`, `output_truncated`, `stack_overflow`, `step_limit_exceeded` (execução passou do cap de linhas steppadas — ver "Throttling de eventos e escopo de tamanho de execução").

> **Assimetria conhecida entre Java e C# no campo `locals` (pós-unificação Fase 0.5):** o schema do evento (`type`, `line`, `locals`, `stack`, `time_ns`, `memory_bytes`) é idêntico nas duas linguagens — mesmo `enum` Rust (`sandbox/src/events.rs`) emite os dois. Mas o **conteúdo das chaves de `locals` difere**: Java (via JDI) resolve o nome real da variável (`"x"`, `"i"`) porque a JVM carrega essa informação de debug nativamente. C# (via ICorDebug direto, sem netcoredbg) ainda usa chaves posicionais (`"local_0"`, `"local_1"`) porque mapear índice→nome exige ler o Portable PDB, e isso foi investigado e propositalmente adiado (ver seção "Estratégia C#" — não existe symbol reader nativo no SDK do .NET, exigiria escrever um parser de Portable PDB do zero). O frontend precisa tratar isso como diferença **interina e conhecida**, não bug: para C#, exibir `local_N` (com fallback pro índice) até o parser de PDB ser implementado (backlog, não MVP).

### Recuperar trace completo

`GET /executions/b3f1c2a4-6e9d-4a2b-9f3e-1d7c8a0b5f6e/trace`

Retorna o array completo de eventos de uma execução já finalizada, para carregar a página depois de pronta ou reconectar sem perder estado — o frontend não depende de ter recebido tudo via WebSocket ao vivo.

### Throttling de eventos e escopo de tamanho de execução

Um loop de milhões de iterações geraria um evento `step` por iteração, o que afogaria o WebSocket e o frontend antes de qualquer timeout entrar em ação. A emissão de eventos precisa ser amostrada/agregada em execuções longas (ex: 1 evento a cada N passos, ou coalescer passos repetidos na mesma linha), em vez de 1:1 com cada linha executada.

**Decisão de escopo (pós-spike Fase 0.5):** o visualizador passo-a-passo é para entender a mecânica de um algoritmo em entradas pequenas/médias — não para rodar entradas de tamanho realista de produção (como ferramentas comparáveis, ex. Python Tutor, já assumem por design). O cap é de **5.000 eventos de step emitidos por execução** (baseado nas taxas medidas: ~360-1.580 ev/s dependendo da complexidade do estado, mantendo a execução instrumentada numa janela de ~3-14s). Ao atingir o cap, o driver desabilita o `StepRequest` (deixa o programa terminar ou bater no timeout normalmente, sem overhead de instrumentação) e emite `step_limit_exceeded`, em vez de travar num timeout sem explicação. A análise estática de Big-O, não o step-through, é o que informa sobre comportamento em escala.

> **Achado do spike (Fase 0.5) — mais grave que o afogamento de eventos:** medido via `com.sun.jdi` com `StepRequest` (protocolo JDWP, que faz round-trip síncrono a cada linha), a taxa real com extração completa de locals/stack ficou em ~1.580 eventos/segundo — várias ordens de magnitude abaixo de execução nativa. Isso significa que qualquer algoritmo processando mais que algumas dezenas de milhares de elementos **nunca termina de rodar sob instrumentação passo a passo dentro de um timeout de execução razoável**. Ataca direto a proposta central do produto (ver complexidade na prática, incluindo O(n²) etc.) pra inputs de tamanho realista.
>
> **3 experimentos controlados pra isolar a causa:** `SUSPEND_ALL` vs `SUSPEND_EVENT_THREAD` não fez diferença nenhuma (~1.580 eventos/s nos dois — esperado, só existe 1 thread no alvo). Remover a extração de locals/stack (só contar eventos) deu ~7.960 eventos/s — a extração de dados é ~80% do custo. Mas mesmo nesse melhor caso possível, o teto de ~8.000 eventos/s ainda é baixo demais: um loop de 100 mil iterações levaria ~25s só de overhead de step, sem extrair nada de útil. **O gargalo é o próprio protocolo JDWP (round-trip síncrono por linha), não a extração de dados nem a suspend policy.**
>
> **Amostragem testada e funciona, mas não elimina o teto:** implementado toggle de amostragem no driver (extrai/emite dados só a cada N eventos; nos outros só resume). N=1 → ~1.580 ev/s, N=10 → ~5.340 ev/s (3,4x mais rápido), N=100 → ~7.170 ev/s — aproximando assintoticamente do teto de ~8.000 ev/s conforme N cresce (resumir a VM a cada linha via JDWP continua acontecendo mesmo quando o evento não é emitido). Amostragem vale a pena implementar (pode ser a diferença entre um algoritmo de 50 mil elementos rodar em 6s ou 30s), mas não resolve entradas muito grandes — pra isso, a única saída identificada é trocar o mecanismo de instrumentação inteiro, de JDI/JDWP pra um agente de bytecode (`java.lang.instrument` + ASM/ByteBuddy) que injeta logging direto no bytecode do alvo, sem round-trip de protocolo — arquitetura bem diferente, precisa de spike próprio, não testado ainda.

### Precisão das métricas

`time_ns` e `memory_bytes` são medidos com o processo sob instrumentação (JDI, ou equivalente por linguagem) — o overhead do próprio debugger, JIT warmup e pausas de GC tornam esses números ruidosos e não diretamente comparáveis ao tempo/memória de uma execução sem debugger. Isso deve ficar explícito na UI (ex: "medido sob instrumentação"), e não ser tratado como benchmark confiável.

### Serialização de variáveis locais

Objetos grandes, com referência circular, ou com `toString()`/`equals()` custom não têm um jeito óbvio de virar o campo `locals` do evento. A estratégia (profundidade máxima + cap de array/campos + detecção de ciclo) foi implementada e validada no spike da Fase 0.5 — ver `sandbox/jdi/Debugger.java`.

> **Validado no spike:** profundidade máxima 3, cap de 20 elementos/campos, detecção de ciclo via `Set` de object IDs por variável (não compartilhado entre variáveis — duas variáveis apontando pro mesmo objeto é aliasing, não ciclo). Testado com objetos com referência circular (`a.next.next` vira `"<ciclo, id=48>"`) e array de 50 elementos (trunca em 20 com indicador de quantos ficaram de fora). Serialização profunda tem custo bem mais alto que a rasa — reforça que amostragem de eventos não é opcional quando `locals` é serializado de verdade.

### Reconexão

Queda de conexão no WebSocket no meio de uma execução não deve perder o trace já gerado. A API guarda o trace completo por `execution_id` (ver `GET /executions/:id/trace`), então reconectar é só buscar o que já foi gerado até agora — não precisa reiniciar a execução do zero.

### Multi-thread

O schema de evento acima assume single-thread (`"stack": ["main"]`). Código Java com múltiplas threads não se encaixa nesse modelo. Decisão pendente: bloquear execuções multi-thread no MVP, ou já modelar `stack` como um mapa por thread (`{ "thread-1": [...], "thread-2": [...] }`).

## Análise estática

```
Código
  ↓
Parser
  ↓
AST da linguagem
  ↓
Adaptador
  ↓
Complexity IR
  ↓
Complexity Engine
  ↓
Time:  O(n²)
Space: O(1)
```

Para múltiplas linguagens, usar parsers existentes, como Tree-sitter, e transformar a AST em uma IR própria de análise.

> Limitação conhecida: inferir complexidade de código arbitrário é fundamentalmente heurístico (early exit, memoização, complexidade amortizada, comportamento dependente de dado tornam o problema indecidível em geral). O Complexity Engine precisa de um estado explícito de "não foi possível determinar", em vez de sempre cravar um O(...) com aparência de certeza.

## Execução dinâmica

```
Código
  ↓
Sandbox
  ↓
Runtime da linguagem
  ↓
Debugger / Instrumentação
  ↓
Eventos
  ├── linha atual
  ├── variáveis
  ├── call stack
  ├── tempo
  ├── memória
  └── stdout/stderr
```

## Segurança

O código do usuário deve ser tratado como código arbitrário e não confiável.

O sandbox deve ter:

- Sem acesso à rede
- Sem privilégios
- Filesystem temporário
- Limite de CPU
- Limite de memória
- Timeout
- Limite de processos
- Limite de output
- Isolamento de namespaces
- seccomp/filtros de syscalls
- Destruição do ambiente após execução

Para maior segurança: microVMs, como Firecracker, em vez de depender somente de containers.

> Docker sozinho não deve ser considerado uma fronteira de segurança suficiente para um serviço público que executa código arbitrário.

> Isolamento de rede não impede abuso de CPU — alguém pode usar o serviço só para queimar ciclos de CPU de graça (mineração, brute-force local, etc.) mesmo sem conseguir exfiltrar nada. Rate limiting por IP/conta é necessário desde o MVP, não só como feature de produto.

> Mensagens de erro/stack trace do runtime podem vazar detalhes internos do sandbox (caminhos, versão de kernel, estrutura de containers) — precisam ser sanitizadas antes de chegar ao frontend, mostrando só o que é relevante ao erro do código do usuário.

> Em escala, o cgroup limita uma execução individual, mas não impede que muitas execuções simultâneas no mesmo host disputem CPU real entre si ("noisy neighbor"). Isso exige scheduling/bin-packing a nível de cluster (não só limite por processo) conforme o número de usuários simultâneos cresce.

### Isolamento de execução: nsjail

A API e o Sandbox Controller rodam dentro de um container Docker comum (deploy padrão). Mas cada **execução de código do usuário** não sobe outro container Docker — o controller faz `fork+exec` de um processo `nsjail`, que aplica namespaces e seccomp-bpf diretamente via syscalls do kernel.

Isso evita dar ao container do controller acesso ao `docker.sock` do host (o que equivaleria a root no host) e reduz o overhead de subir um container Docker por execução — o `nsjail` não depende de daemon nem de socket, é um único binário.

```
Sandbox Controller (dentro de um container Docker comum)
        │
        │  fork + exec
        ▼
     nsjail
        │
        ├── namespaces (pid, mount, net, uts)
        ├── seccomp-bpf (filtro de syscalls)
        ├── cgroups (CPU, memória, processos)
        └── rootfs efêmero
        │
        ▼
  Runtime da linguagem (isolado)
```

> Ambiente local: todo o projeto sobe via `docker-compose` (API, Sandbox Controller, Frontend). O container do Sandbox Controller precisa rodar com as capabilities que o `nsjail` exige pra criar namespaces (ex: `CAP_SYS_ADMIN`) — sem isso o compose sobe normalmente, mas o isolamento simplesmente não funciona em dev.

### Timeout de execução

Como o modelo é trace-and-replay (a sandbox roda do início ao fim sem pausar esperando o usuário), um timeout único de wall-clock é suficiente — o `--time_limit` do próprio `nsjail` mata o processo depois de N segundos configuráveis. Não existe risco de matar uma execução "pausada num breakpoint", porque nada fica pausado: o breakpoint é só um marcador que o frontend usa pra parar de scrollar o trace, o processo real já rodou até o fim (ou até o timeout).

Ao estourar, emite evento `timeout` com o trace parcial gerado até aquele ponto — ainda útil pro usuário ver onde travou.

### Hardening de memória

O limite de segurança real é o cgroup (`memory.max`, `memory.swap.max=0`, `pids.max`), aplicado ao jail inteiro (todos os processos/threads descendentes), não por PID isolado — isso mitiga fork bombs e truques de multi-thread. Limites de heap por runtime (`-Xmx` no Java, `DOTNET_GCHeapHardLimit` no .NET, etc.) são uma 2ª linha de defesa, pra falhar rápido e limpo antes do `SIGKILL` do cgroup.

> **Validado no spike (Fase 0.5):** em Docker Desktop/macOS (VM Linux aninhada), `cgroup_mem_max` não matava o processo (só o `-Xmx` segurava). Revalidado num Linux real (VM Lima, sem Docker de permeio) e o cgroup funcionou corretamente — matou a JVM antes mesmo de conseguir imprimir qualquer output com um limite de 32MB, e no meio da execução com um limite de 150MB. Confirma que o problema era específico do ambiente de dev local (Docker Desktop), não do desenho — em produção (Linux real), o cgroup é a fronteira confiável como descrito acima. Detalhe completo no backlog (`tasks.md`).
>
> **Confirmado também pra C# (preparo do ambiente, Fase 1):** o CoreCLR reproduz o mesmo padrão da JVM — sem `DOTNET_GCHeapHardLimit` explícito, ele tenta reservar memória virtual demais e falha na inicialização (`GC heap initialization failed`) mesmo com `rlimit_as` bem generoso (testado até 4GB). Isolado por experimento: `DOTNET_gcServer=0` (GC workstation) sozinho não resolve; `DOTNET_GCHeapHardLimit` sozinho resolve — confirma que essa é de fato a mitigação certa, sem precisar mexer no modo de GC.

## Estratégia de desenvolvimento

Java e C# entram juntos no MVP inicial (não mais sequencial). Java valida o desenho via JDI (Java Debug Interface); C# via `netcoredbg` (debugger CoreCLR open-source, mesmo usado pela extensão de C# do VS Code em versões antigas), driblado programaticamente via DAP (Debug Adapter Protocol — JSON sobre stdio).

> **Spike C# — decidido: `netcoredbg` via DAP.** `--interpreter=mi` (protocolo GDB/MI) falhou (o processo debuggee nunca chegava a rodar, erro `0x80004005`); `--interpreter=vscode` (DAP) funcionou de ponta a ponta *fora do nsjail*: breakpoint, step, variáveis, call stack e stdout, tudo capturado com dados reais. Throughput ~948 eventos/s com extração completa (mesma ordem de grandeza do JDI, ~1.580 ev/s — as mitigações de amostragem/cap decididas pro Java se aplicam igual aqui). Cold start ~80-93ms, bem mais rápido que o do Java (~470ms) — não existe aqui o problema de "2ª JVM fazendo handshake": um único processo `netcoredbg` controla tudo.
>
> **Achado crítico dentro do nsjail:** o CoreCLR usa uma técnica de GC chamada "double mapping" que cria um `memfd` e tenta `ftruncate` pra 2TB (reserva de endereço virtual, não disco real) — e o `RLIMIT_FSIZE` default do nsjail é 1MB, matando o processo com `SIGXFSZ` antes dele conseguir rodar. **Fix: `--rlimit_fsize inf`.** Sem isso, C# sob nsjail simplesmente não funciona, independente de qualquer ajuste de memória. Rastreado via `strace -f` depois de várias hipóteses de namespace/capability não confirmarem nada.
>
> **2º achado, também resolvido:** `/tmp` somente-leitura bloqueava o handshake de debug do CoreCLR — `bind()` do socket `/tmp/dotnet-diagnostic-*` e `mknodat()` dos pipes `/tmp/clr-debug-pipe-*` (mecanismo de anexo do debugger via dbgshim) falhavam com `EROFS`. Fix: `--tmpfsmount /tmp` (montar `/tmp` como tmpfs gravável dentro do jail).
>
> **3º problema, revalidado num Linux real (VM Lima) — NÃO é específico do Docker Desktop.** Diferente do caso do `cgroup_mem_max` (que era mesmo uma peculiaridade do Docker Desktop/macOS), essa condição de corrida no handshake do `netcoredbg` reproduz igual num Linux real: mesmo travamento até o timeout, sem nunca completar `configurationDone`. Pesquisa (web) não encontrou correção documentada pra esse combo específico (netcoredbg + sandbox tipo nsjail/seccomp) — `ptrace`/Yama foi descartado com evidência direta do `strace` (dbgshim no Linux não usa `ptrace` pro handshake inicial).
>
> **Pivô confirmado: ICorDebug via interop direto, sem passar pelo `netcoredbg`.** FFI direto pro `libdbgshim.so` (via `libloading`, sem bindings estáticos) — `CreateProcessForLaunch` → `RegisterForRuntimeStartup` → `ResumeProcess` — testado 4x dentro do nsjail com os mesmos fixes de memória/tmp: **4/4 sucessos, callback do runtime disparando em ~50-56ms consistentes, sem travamento**. Confirma que o problema estava numa camada extra do `netcoredbg` sobre o dbgshim, não no CoreCLR em si. C# passa a usar ICorDebug via interop direto em vez de `netcoredbg`/DAP.
>
> **Handshake completo implementado e validado dentro do nsjail.** `QueryInterface(IID_ICorDebug)` → `Initialize()` → `SetManagedHandler(callback próprio)` → `DebugActiveProcess(pid)` → o runtime chamou de volta nosso `CreateProcess` com ponteiro válido — comportamento idêntico dentro e fora do jail, sem travar. A vtable de `ICorDebugManagedCallback` (29 slots: 3 de `IUnknown` + 26 métodos do `cordebug.idl`) está implementada em Rust puro (`sandbox/src/com.rs`), sem nenhuma lib de binding COM. Isso elimina o risco arquitetural (a condição de corrida do netcoredbg).
>
> **Ciclo de vida completo validado**: com `ICorDebugController::Continue()` implementado e chamado após cada callback de carregamento, o processo percorre `CreateProcess → CreateAppDomain → LoadAssembly/LoadModule (múltiplos) → CreateThread → execução real (stdout) → ExitProcess` inteiramente dentro do nsjail, sem travar.
>
> **Breakpoint real funcionando.** `ICorDebugModule::GetName` identifica o módulo do usuário (`Loop.dll`) entre os módulos carregados (CoreLib, System.Runtime, System.Console...); `ICorDebugModule::GetFunctionFromToken(0x06000001)` + `ICorDebugFunction::CreateBreakpoint` criam o breakpoint no `Main` — a convenção de token `0x06000001` se confirmou correta na prática. O breakpoint disparou de verdade: o programa não chegou a imprimir nada, ficou parado exatamente no ponto esperado, dentro do nsjail.
>
> **Pipeline completo validado: step + variável com tipo correto.** `ICorDebugThread::CreateStepper` + `Step(bStepIn=TRUE)` → `StepComplete` disparou → `GetActiveFrame` → `QueryInterface(ICorDebugILFrame)` → `GetLocalVariable(0)` → tipo retornado foi `ELEMENT_TYPE_I4` (bate exatamente com `int x` do código) → valor lido sem erro. **Isso fecha a validação de toda a cadeia necessária pro MVP de C#** — attach sem condição de corrida, ciclo de vida, breakpoint, step, extração de variável com tipo correto, tudo dentro do sandbox. Call stack com múltiplos frames também já validado (`ICorDebugFrame::GetCaller`, mesma vtable de `ICorDebugILFrame` — sem interface nova).
>
> **Nomes de método via `IMetaDataImport` — funcionou sem crash.** Interface bem maior que as anteriores (~60 métodos; `GetMethodProps` é o slot 30, exigindo acertar 27 slots anteriores só pela ordem/contagem, sem poder testar cada um isoladamente). Resultado: `<Main>$` — o nome exato que o Roslyn gera pro método de entrada com top-level statements, confirmando que a vtable estava certa de primeira. **Token do método já é achado de forma robusta**: `IMetaDataImport::EnumTypeDefs`+`EnumMethods` percorrem a assembly procurando `Main`/`<Main>$`, validado contra um programa com uma classe `Helper` (3 métodos) declarada antes do `Main` — achou corretamente um token diferente (`0x06000005`, não mais `0x06000001`), confirmando que não é coincidência.
>
> **Dereferenciar string também funciona.** `ICorDebugReferenceValue::IsNull`→`Dereference`→`QueryInterface(ICorDebugStringValue)`→`GetString` extraiu o conteúdo exato (`"ola mundo"`). Os GUIDs recuperados de memória estavam errados (`QueryInterface` falhou limpo, sem crash — confirma que erro de IID é seguro, diferente de erro de vtable); baixamos o `cordebug.idl` real do `dotnet/runtime` no GitHub pra confirmar os valores certos, e a vtable (ordem dos métodos) já estava correta de memória. **Dereferenciar array também funciona**: `ICorDebugArrayValue` (`GetCount`/`GetElementAtPosition`) testado com `int[] numeros = {10, 20, 30}` — extraiu `[10, 20, 30]` com o tipo de elemento correto.
>
> **PDB investigado, sem atalho disponível.** O SDK do .NET não tem nenhuma lib nativa de symbol reader (`ISymUnmanagedReader`), e `libdbgshim.so` não exporta nada de PDB. O próprio `netcoredbg` carrega Roslyn (`Microsoft.CodeAnalysis.CSharp.dll`) + um `ManagedPart.dll` — evidência de que leitura de Portable PDB moderna é via código gerenciado (`System.Reflection.Metadata`), não COM nativo. Decisão: não seguir por esse caminho agora — seria escrever um parser de Portable PDB do zero (domínio de parsing binário, diferente do padrão de interop COM usado no resto do C#). **Limitação conhecida, não bloqueante**: sem PDB, locals/call stack ficam com índice numérico + tipo em vez de nome de variável, e sem número de linha do C# original (só o offset de IL). O pipeline de debug do C# está completo e validado pra tudo além disso: attach, ciclo de vida, breakpoint, step, extração de locals (primitivos, string, array), call stack com nome de método.

> **Medido no spike (Fase 0.5) — a causa não é bem o que se imaginava:** uma JVM sozinha, sem JDI, sobe e roda um programa trivial em ~18ms dentro do nsjail — nada lento. O custo real está no handshake do `LaunchingConnector`/JDWP (2ª JVM sendo lançada em modo debug + conexão do debugger): ~470ms médios, consistentes em 5 execuções. Ou seja, não é "cold start de JVM" no sentido genérico (JIT warmup, carregamento de classes) — é especificamente o setup da sessão de debug. Isso muda a mitigação: um warm pool de JVMs *simples* não ataca esse custo, porque o gargalo é abrir uma nova sessão JDWP a cada execução, não subir a JVM em si. Precisa investigar se dá pra manter uma sessão de debug já conectada e só trocar o código-alvo entre execuções (tensiona com o modelo de sandbox descartável por execução), ou se o custo de handshake é aceitável por já ser < 500ms fixo por execução.

```
Java + C# (MVP inicial, juntos)
  ↓
Ruby
```

A arquitetura não deve depender de uma linguagem específica.

> Limitação conhecida: JDI (Java), a debugger API do .NET e o `TracePoint` do Ruby não param no mesmo nível de granularidade — a UX de "andar linha por linha" pode ficar inconsistente entre as 3 linguagens. Vale normalizar a granularidade de step na camada de adaptador, em vez de expor a diferença crua de cada runtime ao frontend.

## Separação fundamental

```
Static Analysis
      │
      ├── AST
      ├── Complexity IR
      └── Big-O

Dynamic Execution
      │
      ├── Sandbox
      ├── Debugger
      └── Runtime Events

              ↓

        Web Visualizer
```
