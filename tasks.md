# Tasks

## Fase 0 — Setup

- [ ] Escolher e configurar monorepo (ex: pnpm workspace na raiz + pastas `api/` [Crystal/shards], `sandbox/` [Rust/cargo], `frontend/`)
- [ ] Configurar CI básico (lint, build, testes)
- [ ] Definir convenções de commit/branch
- [ ] `docker-compose.yml` na raiz subindo tudo com um comando: API (Crystal), Sandbox Controller (Rust), Frontend (Angular) e dependências (ex: Redis, se usado pra fila/estado)
- [ ] Dockerfile por serviço (`api/`, `sandbox/`, `frontend/`) com hot-reload em dev, pra não precisar rebuildar a imagem a cada mudança
- [ ] Garantir que o container do Sandbox Controller sobe com as capabilities que o `nsjail` precisa pra criar namespaces (ex: `CAP_SYS_ADMIN`) — sem isso o compose sobe mas o isolamento não funciona localmente
- [ ] `.env.example` com as variáveis necessárias (portas, contrato IPC entre API e Controller, etc.)

## Fase 0.5 — Spike do Sandbox (sem API, sem frontend)

Objetivo: validar se nsjail + JDI dão conta do que o produto precisa, **antes** de comprometer API/frontend a esse desenho. Sem servidor, sem WebSocket — só um binário/CLI Rust que roda um `.java` local e imprime os eventos capturados no terminal.

- [x] CLI que recebe um arquivo `.java`, roda via `fork+exec` de `nsjail` (implementado em `sandbox/`)
- [x] Driver JDI (`sandbox/jdi/Debugger.java`) via `LaunchingConnector` (debugger e alvo no mesmo processo/jail — separação debugger confiável / alvo isolado ainda em aberto): breakpoint na 1ª linha de `main` + `StepRequest` a partir dali (criar o step direto no `ClassPrepareEvent` NÃO funciona — a thread ainda não está dentro do método; é preciso um breakpoint primeiro), com filtro de exclusão pra não entrar em código interno da JVM (`java.*`, `jdk.*`, `sun.*`). Emite linha, `locals` (nome+valor) e call stack em JSON por evento
- [x] Bateria de snippets de teste, cobrindo os casos que mais preocupam antes de seguir:
  - [x] Loop simples (`test-snippets/Loop.java`) — rodou isolado, com JDI capturando linha/variáveis/stack corretamente a cada iteração
  - [x] Recursão profunda (`test-snippets/DeepRecursion.java`) — `StackOverflowError` tratado de forma limpa pela própria JVM, exit 1, sem precisar de `RLIMIT_STACK` como defesa primária (mesmo padrão do `-Xmx`: limite de linguagem resolve antes do SO)
  - [x] Loop infinito (`test-snippets/InfiniteLoop.java`) — `--time_limit 10` matou no segundo exato (SIGKILL, exit 137)
  - [x] Alocação de memória grande/crescente (`test-snippets/MemoryHog.java`) — **achado crítico**: `cgroup_mem_max` (32MB testado) NÃO matou o processo mesmo com `memory.max` corretamente escrito e o PID corretamente no cgroup (verificado via `cgroup.procs`) — só o `-Xmx256m` barrou, aos ~250MB reais alocados. Ambiente: Docker Desktop/macOS (VM Linux aninhada) — precisa revalidar num host Linux real antes de confiar no cgroup como fronteira de segurança
  - [x] Objeto com referência circular / estrutura grande (`test-snippets/CircularRef.java`) — inicialmente só validava `toString()` raso (sem risco de ciclo, mas também sem mostrar valor real). **Atualizado depois**: serialização profunda implementada e testada (ver item da Fase 1 "Serialização profunda de `locals`") — `a.next.next` corretamente vira `"<ciclo, id=48>"`, array grande trunca em 20 elementos
  - [x] Multi-thread (`test-snippets/MultiThread.java`) — funciona normalmente, não precisa bloquear no MVP por questão de isolamento. Só precisou de `rlimit_as` mais generoso (3072MB) — com 2048MB o `Thread.start()` falhava com `OutOfMemoryError: unable to create native thread`, o que não tem nada a ver com nproc/nofile (também testados e descartados como causa), reforça que `RLIMIT_AS` é difícil de calibrar pra JVM
  - [x] Tentativa de acesso à rede (`test-snippets/NetworkEscape.java`) — bloqueada corretamente por padrão (`SocketException: Network is unreachable`), sem precisar de flag extra
  - [x] Tentativa de acesso a filesystem (`test-snippets/FilesystemEscape.java`) — **confirma gap já conhecido**: leu `/etc/shadow` com sucesso, porque ainda usamos `--chroot /` (reaproveita o filesystem do container, sem rootfs isolado — item já previsto na Fase 1/2). Escrita fora do cwd foi bloqueada (`Read-only file system`) pelo comportamento padrão do nsjail de montar a chroot como somente-leitura
- [x] **Revalidado num Linux real (VM Lima, kernel 7.0, sem Docker de permeio)**: `cgroup_mem_max` funciona corretamente — com 32MB de limite, matou a JVM antes de conseguir imprimir qualquer linha (nem chegou a rodar); com 150MB, matou por volta de ~90-100MB real alocado. **Confirma que o problema era específico do Docker Desktop/macOS** (VM Linux aninhada com delegação de cgroup quebrada), não do desenho em si — o modelo do `spec.md` (cgroup como limite real) está correto para deploy em produção (Linux real/cloud)
- [ ] Capabilities mínimas (sem root completo): testado `cap_sys_admin,cap_sys_chroot,cap_net_admin,cap_sys_resource,cap_setuid,cap_setgid` via `setcap` num Linux real — ainda insuficiente (falha em `setgroups` do user namespace). Rodar como root (ou `--privileged` equivalente) funciona; achar o conjunto mínimo exato de capabilities fica pra Fase 2 (não é bloqueante pro spike, só relevante pra hardening)
- [x] `rlimit_nproc`/`rlimit_nofile` não eram a causa das falhas de thread — era `rlimit_as` baixo demais (resolvido subindo pra 3072MB); mantidos altos (512 e 1024) por segurança, mas a variável que realmente importa calibrar é `rlimit_as`
- [x] Rodar debugger (JDI) + alvo como 2 JVMs (`LaunchingConnector`) quase dobra a reserva de memória virtual — cada JVM reserva ~1GB de compressed class space por padrão; precisou de `-XX:CompressedClassSpaceSize=64m` explícito nas duas pra caber no `rlimit_as`
- [x] **Medido overhead real do JDI — achado crítico, mais grave que o afogamento de eventos**: ~1.580 eventos/segundo sob `StepRequest` com extração completa de locals/stack (medido com `test-snippets/BigCountLoop.java`, 20000 iterações). Qualquer algoritmo com mais de algumas dezenas de milhares de elementos **não termina de rodar dentro de um timeout de execução razoável** sob instrumentação passo a passo. Ataca a proposta central do produto pra inputs de tamanho realista
- [x] **3 experimentos controlados pra achar a causa raiz** (toggle via env vars `SPIKE_SUSPEND`/`SPIKE_SKIPDATA` no driver): (A) `SUSPEND_ALL` + extração completa = ~1.584 ev/s; (B) `SUSPEND_EVENT_THREAD` + extração completa = ~1.577 ev/s (suspend policy não faz diferença, só 1 thread no alvo); (C) `SUSPEND_ALL` sem extrair locals/stack = ~7.959 ev/s (extração é ~80% do custo, mas o teto de ~8.000 ev/s mesmo sem extrair nada mostra que **o gargalo real é o round-trip síncrono do protocolo JDWP em si**, não a extração de dados nem a suspend policy)
- [x] **Cold start medido com precisão (5 execuções, `test-snippets/Trivial.java`)**: ~470ms médios (458-485ms) pro pipeline completo. Comparado com JVM sozinha sem JDI no mesmo container (~18ms) — **a causa não é o boot da JVM, é o handshake do `LaunchingConnector`/JDWP** (2ª JVM em modo debug + conexão do debugger). Corrige a suposição anterior no `spec.md` de que seria "cold start de JVM" genérico — muda a mitigação: warm pool de JVMs simples não resolve, porque o custo é abrir uma sessão de debug nova a cada execução, não o boot em si
- [x] **Amostragem de step prototipada e testada** (flag `-Dspike.sample=N` no driver: extrai/emite dados só a cada N eventos, resume sem extrair nos outros). N=1 → ~1.580 ev/s, N=10 → ~5.340 ev/s (3,4x mais rápido), N=100 → ~7.170 ev/s, aproximando do teto de ~8.000 ev/s conforme N cresce. **Funciona e vale implementar** (pode ser a diferença entre 6s e 30s pra 50 mil elementos), mas não resolve entradas muito grandes — o resume da VM a cada linha via JDWP acontece de qualquer forma, mesmo sem emitir o evento
- [ ] Mitigação estrutural pra entradas *muito* grandes fica como ideia futura (não bloqueante): trocar JDI/JDWP por agente de bytecode (`java.lang.instrument` + ASM/ByteBuddy) sem round-trip de protocolo — não entra no MVP, só se o cap de escopo abaixo se mostrar insuficiente na prática
- [x] **Cap de eventos decidido: 5.000 eventos de step emitidos por execução.** Baseado nas taxas medidas (~360 ev/s com objetos complexos, ~1.580 ev/s com variáveis simples), isso mantém a execução instrumentada dentro de ~3-14s dependendo da complexidade do estado — janela aceitável de espera. Ao atingir o cap: para de emitir `step` (desabilita o `StepRequest`, deixa o programa rodar solto até terminar ou bater no timeout de execução) e emite `step_limit_exceeded`. Amostragem (N fixo, valor a calibrar na Fase 1) reduz a chance de bater nesse cap achatando o custo por evento; amostragem adaptativa/baseada em mudança de variável fica pra depois

## Decisão de ir/não ir (Fase 0.5) — **GO, com escopo explícito**

Segue pra Fase 1 com nsjail + JDI. O teto de ~8.000 eventos/s do protocolo JDWP é uma limitação física real, não um bug a corrigir — a decisão é tratar isso como **restrição de produto**, não como pendência técnica: o visualizador passo-a-passo é para entender a *mecânica* de um algoritmo em entradas pequenas/médias (como ferramentas comparáveis, ex. Python Tutor, já fazem por design), não para rodar entradas de tamanho realista de produção. A análise estática de Big-O (não o step-through) é o que informa sobre comportamento em escala.

Consequência direta pro design: a API e o frontend precisam comunicar esse limite explicitamente ao usuário (ex: "essa execução tem mais de N passos, mostrando os primeiros N" ou similar), em vez de deixar implícito ou deixar a execução travar num timeout sem explicação.

## Fase 1 — MVP com Java + C#

Java e C# entram juntos (não mais sequencial). Ordem: Sandbox Runner primeiro (Java evolui direto do spike da Fase 0.5; C# precisa de spike próprio antes de implementar de verdade) → Static Analyzer → API → Frontend, só depois de tudo abaixo validado.

### Ambiente C# preparado (isolamento validado, sem instrumentação ainda)

- [x] .NET 8 SDK instalado na imagem Docker do sandbox (`sandbox/Dockerfile`), via instalador oficial da Microsoft (`dotnet-install.sh`) — não depende de pacote apt, mesmo padrão usado pro Rust via rustup
- [x] Suprimir ruído de "first-run" do .NET (`DOTNET_NOLOGO=1`, `DOTNET_CLI_TELEMETRY_OPTOUT=1`) — sem isso, toda execução isolada mostraria a mensagem de boas-vindas/telemetria, já que cada jail é efêmero e não tem estado persistente entre execuções
- [x] Validado fluxo real de execução: `dotnet build` uma vez (~1,5s) gera um `.dll`, depois `dotnet <dll>` roda em ~35ms — não usar `dotnet run` em produção (isso reinvoca o MSBuild a cada execução)
- [x] Isolamento via nsjail testado com `test-snippets-csharp/Loop` e `InfiniteLoop`: mesmo padrão de falha do Java (`GC heap initialization failed`, CoreCLR tentando reservar memória virtual demais e estourando `rlimit_as`, mesmo com 4GB de folga). Isolado com 2 experimentos controlados: `DOTNET_gcServer=0` (GC workstation) sozinho **não resolve**; `DOTNET_GCHeapHardLimit` sozinho **resolve** — confirma exatamente a mitigação já prevista no hardening de memória (`DOTNET_GCHeapHardLimit`), não precisa dobrar aposta em GC workstation
- [x] Timeout (`--time_limit`) validado com `InfiniteLoop` — matou em ~10.007ms (SIGKILL, exit 137), mesmo comportamento do Java
- [x] **Spike do mecanismo de instrumentação C# — decidido: `netcoredbg` via DAP.** ICorDebug via interop nem chegou a ser tentado (`netcoredbg` já se mostrou viável e é bem menos trabalhoso que driblar uma API COM nativa).
  - [x] `netcoredbg` instalado na imagem (release `3.0.0-1006` — a "latest" exige glibc 2.38+, incompatível com o glibc 2.36 do Debian bookworm; precisou pinar uma versão anterior)
  - [x] **`--interpreter=mi` (protocolo GDB/MI) falhou** — `-exec-run` retornava `Error: 0x80004005` e o log mostrava `child process stdout/stderr reading error` (o processo debuggee nunca chegava a rodar de verdade). Não investigado a fundo — trocou-se direto pra DAP
  - [x] **`--interpreter=vscode` (DAP) funciona de ponta a ponta**: `initialize` → `launch` → `setBreakpoints` → `configurationDone` → evento `stopped` no breakpoint → `stackTrace`/`scopes`/`variables` capturam linha e variáveis reais → `next` avança o step → `continue` retoma → eventos `output` capturam stdout. Testado com Python driblando o protocolo via stdio (JSON com header `Content-Length`), fora do nsjail ainda
  - [x] **Throughput medido**: ~948 eventos/s com extração completa de stack+scopes+variables por evento (medido com `BigCountLoop`, 2000 iterações) — mesma ordem de grandeza do JDI (~1.580 ev/s), não catastroficamente pior. As mitigações já decididas pro Java (amostragem, cap de 5.000 eventos) devem se aplicar igual aqui
  - [x] **Cold start medido**: ~80-93ms (5 execuções) do lançamento do processo até o primeiro breakpoint — bem mais rápido que os ~470ms do Java. Não tem o problema de "2ª JVM fazendo handshake JDWP": um único processo `netcoredbg` controla tudo, sem precisar lançar um segundo processo em modo debug separado
  - [x] **Testado dentro do nsjail — achado crítico e resolvido**: `configurationDone` falhava sempre (`0x80004005`) e o processo debuggee morria com `SIGKILL`/`SIGXFSZ` antes de produzir qualquer output. Rastreado via `strace -f`: o CoreCLR cria um `memfd_create("doublemapper", ...)` (técnica de double-mapping do GC) e tenta `ftruncate` pra **2TB** (reserva de endereço virtual, não disco real de verdade) — e o default do `nsjail` pra `RLIMIT_FSIZE` é **1MB**, matando o processo com `SIGXFSZ` na hora. **Fix confirmado: `--rlimit_fsize inf`** (sem isso, C# sob nsjail nunca funciona, independente de qualquer outro ajuste de memória/cgroup)
  - [x] **2º achado crítico e resolvido: `/tmp` somente-leitura bloqueia o handshake de debug do CoreCLR.** Rastreado via `strace -f`: `bind()` do socket `/tmp/dotnet-diagnostic-*` e `mknodat()` dos pipes `/tmp/clr-debug-pipe-*-in/out` (mecanismo clássico de anexo de debugger do CoreCLR/dbgshim) falhavam com `EROFS`. **Fix confirmado via strace: `--tmpfsmount /tmp`** (monta `/tmp` como tmpfs gravável dentro do jail) — com isso, `bind`/`mknodat` retornam sucesso e os pipes são criados
  - [x] **3º problema — revalidado num Linux real (VM Lima), reproduziu igual: NÃO é específico do Docker Desktop.** Diferente do caso do `cgroup_mem_max` (que era mesmo um problema só do Docker Desktop/macOS), essa condição de corrida no handshake do `netcoredbg` acontece igual num Linux real — mesmo travamento até o timeout, sem nunca completar `configurationDone`. É um problema genuíno de compatibilidade entre `netcoredbg`/dbgshim e o isolamento de namespaces do nsjail, não uma peculiaridade de ambiente de dev
  - [x] **Pesquisado (web) antes de continuar advinhando**: ordem de comandos DAP já está correta (bate com a que o VS Code usa); `ptrace`/Yama descartado com evidência direta do `strace` (nenhuma chamada `ptrace`/`process_vm_readv` acontece — o dbgshim no Linux não depende disso pro handshake inicial); nenhum issue público encontrado sobre "netcoredbg + nsjail" ou "netcoredbg + seccomp sandbox" especificamente — combinação pouco testada publicamente
  - [x] **PIVÔ CONFIRMADO — ICorDebug via interop direto funciona, sem a condição de corrida do netcoredbg.** Implementado `sandbox/src/icordebug_spike.rs`: chama `libdbgshim.so` direto via FFI (`libloading`, sem precisar de bindings estáticos) — `CreateProcessForLaunch` → `RegisterForRuntimeStartup` → `ResumeProcess` → callback do runtime. Testado 4x dentro do nsjail (com os mesmos fixes de `rlimit_fsize inf` + `tmpfsmount /tmp`): **4/4 sucessos, callback disparando em ~50-56ms consistentes**, sem nenhum travamento. Prova que o problema estava numa camada extra do `netcoredbg` sobre o dbgshim, não no CoreCLR/dbgshim em si — o caminho de ICorDebug direto é viável e não herda essa condição de corrida
  - [x] **Handshake completo do ICorDebug implementado e validado dentro do nsjail — sem a condição de corrida do netcoredbg.** `sandbox/src/com.rs`: plumbing COM mínimo em Rust puro (structs `#[repr(C)]` pra vtables, ABI Itanium C++), incluindo `IUnknown::QueryInterface`, `ICorDebug` (Initialize/SetManagedHandler/DebugActiveProcess) e uma implementação completa de `ICorDebugManagedCallback` (29 slots de vtable — os 3 de `IUnknown` + 26 métodos do `cordebug.idl`, todos implementados/logados). Sequência testada dentro do nsjail: `QueryInterface(IID_ICorDebug)` → `Initialize()` → `SetManagedHandler(nosso callback)` → `DebugActiveProcess(pid)` → **o runtime chamou de volta nosso `CreateProcess` com um ponteiro de processo válido** — idêntico ao comportamento fora do jail, sem travar. Prova que a vtable está correta (um layout errado teria crashado) e que o caminho ICorDebug direto é 100% viável no ambiente sandboxed
  - [x] **`ICorDebugController::Continue()` implementado e testado — ciclo de vida completo validado dentro do nsjail.** Chamado após cada callback "de carregamento" (`CreateProcess`, `CreateAppDomain`, `LoadAssembly`, `LoadModule`, `LoadClass`, `CreateThread`, `NameChange`), todos retornando `hr=0x00000000`. Sequência observada de ponta a ponta: `CreateProcess → CreateAppDomain → LoadAssembly/LoadModule (múltiplos, incluindo System.Private.CoreLib) → CreateThread → programa roda e produz stdout de verdade → CreateThread → ExitProcess`. Zero travamentos, zero crashes, tudo dentro do nsjail — igual ao comportamento esperado de um debugger de verdade
  - [x] **BREAKPOINT REAL FUNCIONANDO — validado dentro do nsjail.** Implementado `ICorDebugModule::GetName` (identifica `Loop.dll` entre os módulos carregados, ignorando CoreLib/System.Runtime/System.Console) e `ICorDebugModule::GetFunctionFromToken` + `ICorDebugFunction::CreateBreakpoint` (mais direto que passar por `ICorDebugCode`). **A convenção de token `0x06000001` pro método `Main` se confirmou correta** — `GetFunctionFromToken` retornou sucesso de primeira. O breakpoint foi criado e **disparou de verdade**: o programa não chegou a imprimir nada (diferente da rodada anterior, que rodava livre até o fim) — ficou parado exatamente no ponto esperado, dentro do nsjail, sem travar
  - [x] **PIPELINE COMPLETO VALIDADO — step + extração de variável com tipo correto, dentro do nsjail.** `ICorDebugThread::CreateStepper` + `ICorDebugStepper::Step(bStepIn=TRUE)` → `StepComplete` disparou → `ICorDebugThread::GetActiveFrame` → `QueryInterface(ICorDebugILFrame)` → `ICorDebugILFrame::GetLocalVariable(0)` → `ICorDebugGenericValue::GetType()` retornou `0x8` (`ELEMENT_TYPE_I4`, exatamente `int`, batendo com `int x` do código) → `GetValue()` leu o inteiro sem erro. **Isso fecha a validação de toda a cadeia necessária pro MVP de C#**: attach sem condição de corrida, ciclo de vida completo, breakpoint real, step real, extração de variável com tipo correto — tudo dentro do sandbox
  - [x] **Call stack com múltiplos frames implementado e testado.** `ICorDebugFrame::GetCaller` (herdado por `ICorDebugILFrame`, mesma posição de vtable — não precisa de interface nova) sobe a pilha chamando `GetFunction`/`GetToken` em cada nível. Testado: `call stack[0]: token=0x06000001` (bate com o token do `Main`), reportou corretamente "topo da pilha, 1 frame no total" (esperado, já que `Main` não tem chamador de código do usuário visível)
  - [x] **Nomes de métodos via `IMetaDataImport::GetMethodProps` — funcionou de primeira, sem crash.** Interface grande (~60 métodos), implementada com os 27 slots anteriores a `GetMethodProps` (índice 30) nomeados individualmente em `com.rs` pra facilitar auditoria. `ICorDebugModule::GetMetaDataInterface` (mais 5 slots novos no `ModuleVtbl`, até o índice 14) + `ICorDebugFunction::GetModule` (slot 3) fecham o caminho até o metadata da assembly. Resultado do teste: call stack mostrou **`<Main>$ (token=0x06000001)`** — o nome exato que o compilador Roslyn gera pro método de entrada com top-level statements, confirmando que a vtable de 27 slots recuperada de memória estava certa de primeira
  - [x] **Token do método achado de forma robusta — validado contra programa com múltiplos tipos/métodos.** `IMetaDataImport::EnumTypeDefs` + `EnumMethods` (mais 3 slots tipados: `close_enum`, `enum_type_defs`, `enum_methods`) percorrem todos os tipos/métodos da assembly procurando `"Main"`/`"<Main>$"`, sem assumir token fixo. Testado com `test-snippets-csharp/MultiMethod` (uma classe `Helper` com 3 métodos definida *antes* do `Main` no código) — achou corretamente **token `0x06000005`** (diferente do `0x06000001` do teste anterior de 1 método só), confirmando que a busca é real, não coincidência. Identificação do módulo do usuário também generalizada (antes hardcoded pra `Loop.dll`, agora é "não está em `/usr/share/dotnet/`")
  - [x] **Dereferenciar string funcionando — conteúdo real extraído.** `ICorDebugReferenceValue::IsNull` → `Dereference` → `QueryInterface(ICorDebugStringValue)` → `GetString`. Os GUIDs recuperados de memória pra `ICorDebugReferenceValue`/`ICorDebugStringValue` estavam **errados** (família certa é `CC7BCAFx`, não `CC7BCAEx`) — `QueryInterface` falhou de forma limpa e segura (`E_NOINTERFACE`, sem crash), então baixei o `cordebug.idl` direto do repositório `dotnet/runtime` no GitHub e confirmei os GUIDs certos; a vtable em si (ordem dos métodos: `IsNull`/`GetValue`/`SetValue`/`Dereference`/`DereferenceStrong`, depois `IsValid`/`CreateRelocBreakpoint`/`GetLength`/`GetString`) já estava certa de memória. Testado com `test-snippets-csharp/StringVar`: `local[0] = "ola mundo"` — conteúdo exato extraído (precisou de 8 steps após o breakpoint pra passar da atribuição, não 3)
  - [x] **Dereferenciar array funcionando — conteúdo real extraído.** `ICorDebugArrayValue` (`GetElementType`/`GetCount`/`GetElementAtPosition`, ordem confirmada no `cordebug.idl` antes de implementar — vtable certa de primeira). Extraído `dereference_value()` como helper compartilhado (`IsNull`+`Dereference`) já que string e array usam o mesmo padrão de referência. Testado com `test-snippets-csharp/ArrayVar` (`int[] numeros = {10, 20, 30}`): `local[0] = [10, 20, 30]` (tipo elemento 0x8 = int) — precisou de 20 steps (inicialização de array com literais gera bem mais IL — `newarr` + `stelem` por elemento — que uma atribuição simples)
  - [x] **Investigado o caminho pra PDB — sem atalho COM disponível, é domínio diferente do resto.** Procurado no SDK do .NET por qualquer lib de symbol reader nativa (`ISymUnmanagedReader`/`diasymreader`) — não existe nenhuma. `libdbgshim.so` não exporta nada relacionado a PDB. O próprio `netcoredbg` carrega `Microsoft.CodeAnalysis.CSharp.dll` (Roslyn) + um `ManagedPart.dll` — evidência de que a leitura de Portable PDB moderna é feita via **código gerenciado** (`System.Reflection.Metadata`), não via API COM nativa. **Decisão**: não seguir por esse caminho agora — seria escrever um parser de Portable PDB do zero (tabelas de metadata comprimidas, ECMA-335 §II.24), domínio de parsing binário bem diferente do padrão de interop COM/vtable usado em todo o resto do C#, sem o mesmo ciclo "implementa → testa → corrige" que funcionou bem até aqui
  - [ ] **Limitação conhecida, não bloqueante pro MVP**: sem o PDB, `locals`/call stack ficam com índice numérico + tipo (ex: `local[0]`) em vez de nome de variável (ex: `x`), e sem número de linha do C# original (só teríamos o offset de IL, se quiséssemos expor). Documentar isso explicitamente pro usuário final, ou revisitar essa investigação depois se virar bloqueio real pro produto (objetos genéricos com campos também ficam pra depois, mesmo padrão de `dereference_value()` já estabelecido caso surja necessidade)
  - [x] **Bateria de segurança do C# fechada — mesmos achados do Java, sem surpresa nova.** `test-snippets-csharp/MemoryHog`: `cgroup_mem_max` (150MB) não segurou (mesma limitação do Docker Desktop já identificada e resolvida pro Java na VM Lima — cgroup é mecanismo de kernel agnóstico de linguagem, não precisa revalidar em Linux real de novo), só `DOTNET_GCHeapHardLimit` (256MB) barrou, com `OutOfMemoryException` limpa (exit 133). `test-snippets-csharp/MultiThreadCs` (8 threads): funciona sem problema, mesmo `rlimit_as` (3072MB) que já usávamos. Timeout já validado antes com `InfiniteLoop`. Referência circular/objetos complexos ficam como item futuro (mesmo padrão de `dereference_value()` já estabelecido, sem urgência)

### Sandbox Runner (Rust)
- [ ] Empacotar/instalar `nsjail` na imagem Docker do controller
- [ ] Wrapper Rust que faz `fork+exec` do `nsjail` por execução (sem depender de `docker.sock`) com:
  - [ ] Sem acesso à rede (`--disable_clone_newnet` ou namespace de rede isolado)
  - [ ] Sem privilégios (usuário não-root dentro do jail)
  - [ ] Filesystem temporário/efêmero (rootfs somente leitura + tmpfs)
  - [ ] Limite de CPU e memória (via cgroups do nsjail)
  - [ ] Timeout de execução (`--time_limit` do nsjail — modelo é trace-and-replay, então wall-clock simples já é suficiente, sem risco de matar execução "pausada")
  - [ ] Limite de processos e de output
  - [ ] seccomp-bpf (allowlist mínima de syscalls por linguagem)
- [ ] Destruição do ambiente (rootfs/tmpfs) após cada execução
- [ ] Runtime Java instrumentado (via JDI — Java Debug Interface) emitindo eventos:
  - [ ] linha atual
  - [ ] variáveis locais
  - [ ] call stack
  - [ ] tempo (ns)
  - [ ] memória (bytes, via JMX/heap)
  - [ ] stdout/stderr
- [ ] Runtime C# instrumentado (mecanismo a definir no spike acima) emitindo os mesmos tipos de evento que o Java
- [ ] Normalizar granularidade de step entre Java (JDI) e C# na camada de adaptador — não expor a diferença crua de cada runtime ao resto do sistema
- [x] Serialização profunda de `locals` implementada e testada no spike (`sandbox/jdi/Debugger.java`, método `serializeValue`): profundidade máxima (3), cap de array/campos (20), detecção de ciclo via `Set` de object IDs por variável top-level (não compartilhado entre variáveis — aliasing não é ciclo). Validado contra `CircularRef.java`: `a.next.next` corretamente vira `"<ciclo, id=48>"`, array de 50 elementos trunca em 20 com `"...(+30 elementos)"`. **Achado**: serialização profunda é bem mais cara que a rasa (~362 ev/s nesse teste vs ~1.580 antes, porque array grande é re-serializado por inteiro a cada step) — reforça que amostragem (já prototipada) é necessária junto, não opcional
- [ ] Decidir modelo de evento para multi-thread: bloquear execuções multi-thread no MVP, ou modelar `stack` por thread desde já
- [ ] Throttling/amostragem de eventos `step` em execuções longas (ex: 1 a cada N passos — já prototipado no spike da Fase 0.5, portar a lógica), para não afogar WebSocket/frontend em loops de milhões de iterações
- [ ] Cap de 5.000 eventos de step emitidos por execução (decidido na Fase 0.5 — ver spec.md), igual pras duas linguagens: ao atingir, emitir `step_limit_exceeded` e desabilitar o step, em vez de deixar rodar até o timeout sem explicação
- [ ] Serialização dos eventos em JSON e envio para a API (fila/pipe)
- [ ] Pool de JVMs pré-aquecidas (warm pool) ou Class Data Sharing (CDS) para reduzir cold start — JVM subindo do zero a cada execução contradiz a promessa de "tempo real"
- [ ] Deixar explícito na UI que `time_ns`/`memory_bytes` são medidos sob instrumentação (overhead do JDI, JIT warmup, pausas de GC tornam os números ruidosos, não um benchmark confiável)

### Static Analyzer
- [ ] Integrar Tree-sitter para parsing de Java e C#
- [ ] Definir esquema da Complexity IR (própria)
- [ ] Adaptador AST (Java) → Complexity IR
- [ ] Adaptador AST (C#) → Complexity IR
- [ ] Complexity Engine: heurísticas iniciais para detectar loops aninhados, recursão simples
- [ ] Estimativa de complexidade temporal (O(n), O(n²), O(log n)...)
- [ ] Estimativa de complexidade espacial (O(1), O(n)...)
- [ ] Estado explícito de "não foi possível determinar" no Complexity Engine, para casos onde a heurística não tem confiança (early exit, memoização, complexidade amortizada, comportamento dependente de dado)

### API (Crystal)
- [ ] Estrutura inicial do serviço em Crystal (framework web: Kemal)
- [ ] `POST /executions` — recebe `language` + `code`, retorna `execution_id` (UUID v4, gerado pela API — não expor IDs sequenciais/previsíveis)
- [ ] `GET /executions/:id/events` — endpoint WebSocket para stream de eventos
- [ ] `GET /executions/:id/trace` — retorna o array completo de eventos de uma execução já finalizada
- [ ] Fila/estado de execuções em memória (ou Redis) para MVP
- [ ] Definir contrato/IPC entre API (Crystal) e Sandbox Controller (Rust) — ex: gRPC, socket unix + JSON, ou fila (Redis/NATS)
- [ ] Armazenar o trace completo por `execution_id` (não só os últimos N eventos), para reconexão sem perder o que já foi gerado

### Frontend (TypeScript + Angular)
- [ ] Estrutura inicial do projeto (Angular CLI, standalone components)
- [ ] Editor de código (Monaco ou CodeMirror)
- [ ] Cliente WebSocket para consumir eventos de execução (buffer local do trace conforme os eventos chegam)
- [ ] Fallback para `GET /executions/:id/trace` — carregar trace completo se a página abrir depois da execução terminar, ou reconectar sem perder estado
- [ ] Navegação client-side (step forward/back, breakpoints) sobre o trace já recebido, sem round-trip pro servidor
- [ ] Painel de execução linha a linha (highlight da linha atual)
- [ ] Painel de variáveis locais
- [ ] Painel de call stack
- [ ] Painel de stdout/stderr
- [ ] Mensagem explícita quando `step_limit_exceeded`/`timeout`/`memory_limit_exceeded` interrompem a execução — explicar o motivo, não deixar a UI travada sem feedback
- [ ] Indicador de complexidade (Big-O) vindo da análise estática
- [ ] Gráfico/timeline de tempo de execução e uso de memória

## Fase 2 — Segurança / Hardening

- [ ] Refinar allowlist de seccomp-bpf por linguagem (reduzir superfície de syscalls ao mínimo necessário)
- [ ] Avaliar migração de nsjail para microVMs (Firecracker) para cargas mais sensíveis
- [ ] Rate limiting e quotas por usuário/IP (isolamento de rede não impede abuso de CPU — precisa existir desde o MVP, não só como feature de produto)
- [ ] Sanitizar mensagens de erro/stack trace do runtime antes de expor ao frontend (podem vazar caminhos internos, versão de kernel, estrutura de containers)
- [ ] Testes de fuga de sandbox (pentest interno, incluindo tentativas de escape do nsjail)
- [ ] Scheduling/bin-packing de execuções a nível de cluster (cgroup limita 1 execução, mas não impede "noisy neighbor" entre várias execuções simultâneas disputando CPU real no mesmo host)

### Timeout de execução

- [ ] Definir o valor do `--time_limit` do nsjail (wall-clock único — modelo trace-and-replay não precisa de RLIMIT_CPU nem timeout de sessão separado, já que a sandbox nunca fica pausada esperando o usuário)
- [ ] Emitir evento `timeout` com o trace parcial gerado até o momento em que a execução foi morta

### Hardening de memória

- [ ] Aplicar `memory.max` (cgroup) no jail inteiro, cobrindo todos os processos/threads descendentes — não só o processo principal (mitiga fork bomb / multi-thread contornando limite por processo)
- [ ] Aplicar `pids.max` / `--rlimit_nproc` para limitar quantidade de processos/threads por execução
- [ ] Desativar swap do cgroup (`memory.swap.max = 0`) para forçar OOM kill imediato em vez de swap thrashing no host
- [ ] Limitar `RLIMIT_AS` (address space) além do `memory.max`, para pegar truques de `mmap`/virtual memory que burlam limite de RSS
- [ ] Configurar limites de heap por runtime como 2ª linha de defesa (fail rápido/limpo antes do `SIGKILL` do cgroup):
  - [ ] Java: `-Xmx`, `-XX:MaxMetaspaceSize`, `-XX:MaxDirectMemorySize` (cobre off-heap/NIO buffers)
  - [ ] C#/.NET: `DOTNET_GCHeapHardLimit`
  - [ ] Ruby: `RUBY_GC_HEAP_GROWTH_MAX_SLOTS` / limite de memória do processo
- [ ] Limitar `RLIMIT_STACK` (tamanho de stack) e capturar overflow de recursão como evento limpo (`stack_overflow`), não como crash genérico
- [ ] Limitar tamanho/rate de output (stdout/stderr) com truncamento + evento `output_truncated`, para não estourar memória do controller/API ao processar output de loop infinito de print
- [ ] Truncar payload de variáveis grandes na serialização de eventos (cap de elementos/bytes por variável), para não estourar memória do instrumentador ao inspecionar arrays/strings enormes
- [ ] Emitir evento `memory_limit_exceeded` quando o cgroup matar a execução por OOM (mesmo padrão do evento `timeout`), para o frontend explicar o motivo ao usuário

## Fase 3 — Expansão de linguagens

- [ ] Ruby
  - [ ] Adaptador AST → Complexity IR
  - [ ] Runtime instrumentado (TracePoint API)
  - [ ] Normalizar granularidade de step do TracePoint contra o padrão já estabelecido por JDI/C# (evitar expor a diferença crua ao frontend)

## Fase 4 — Produto

- [ ] Persistência de execuções (histórico por usuário)
- [ ] Autenticação/contas
- [ ] Compartilhamento de execuções (link público)
- [ ] Métricas de uso e observabilidade (logs, tracing)
- [ ] Documentação pública da API
