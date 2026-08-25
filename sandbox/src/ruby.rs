// Runtime Ruby (TracePoint API) — ao contrário de Java (JDI, processo alvo
// separado falando JDWP) e C# (ICorDebug, interop COM direto de dentro do
// nosso próprio binário), aqui a instrumentação roda dentro de um ÚNICO
// processo `ruby`: sandbox/ruby/driver.rb faz `load` do script do usuário
// DENTRO do bloco de tracing do TracePoint, no mesmo processo. nsjail só
// precisa isolar 1 processo Ruby, mais simples que os dois casos anteriores
// (sem 2º processo pra reaping/OOM-attribution, sem self-re-exec).
//
// driver.rb já emite o mesmo schema JSON de `events::Event` diretamente no
// stdout (ver o comentário de módulo desse arquivo) -- este módulo só monta
// a invocação do nsjail e delega detecção de timeout/OOM/output-cap pra
// `events::run_nsjail`, igual java.rs/csharp.rs.

use std::env;
use std::path::Path;
use std::process::Command;

use crate::events::{self, Event, RunOutcome};

// Fase 3 hardening: minimal default-deny seccomp-bpf allowlist pro processo
// `ruby driver.rb <script>` jailed. Kafel syntax, mesma verificação já feita
// pra JAVA_SECCOMP_POLICY/CSHARP_SECCOMP_POLICY (`nsjail --help` dentro da
// imagem real, mesmo fork do nsjail/kafel).
//
// Derivado da MESMA forma que as duas linguagens anteriores, mas desta vez
// com o passo de derivação feito e documentado ANTES de escrever este
// arquivo (não depois): `strace -f ruby driver.rb <script>` rodado num
// container debian:bookworm-slim com `apt-get install ruby` FORA de
// qualquer jail (mesma razão de sempre -- o chroot read-only do nsjail
// mascara falhas de syscall como "teria sido bloqueado de qualquer jeito"),
// união de 9 probes cobrindo: loop/while, recursão direta, bloco
// `.each`/array, hash, exceção não capturada, overflow de pilha real
// (STEP_EVENT_CAP + SystemStackError de verdade), flood de output em loop
// infinito, `sleep`+`Array.new`+`GC.start`, tentativa de leitura de
// `/etc/shadow`+escrita fora do workdir+`File.symlink`/`.rename`/`.chmod`/
// `.utime`/`.link` (mesmo grupo do achado de pentest do Java/C#, ver
// abaixo), e abertura de socket TCP cliente + servidor
// (`TCPSocket.new`/`TCPServer.new`). Revalidado depois rodando de verdade
// DENTRO do nsjail já com esta política (ver tasks.md pro resultado real).
//
// Achado estrutural, confirmado por strace e não por suposição: o MRI
// (Ruby 3.1, pacote `ruby` do Debian bookworm) usa uma timer POSIX
// (`timer_create`/`timer_settime`, entregue como sinal) pro mecanismo de
// checagem de interrupção entre instruções, NÃO uma 2ª thread real
// (`clone`/`wait4` nunca apareceram em nenhum dos 9 probes, mesmo nos que
// alocam/iteram bastante) -- mais simples que o timer thread real da JVM
// ou o thread pool do CoreCLR, e não precisa de `clone`/`wait4` aqui
// (removidos deliberadamente do primeiro rascunho desta política, que
// tinha copiado o grupo do Java por analogia sem essa validação -- ficou
// comprovadamente desnecessário). `futex`/`ppoll`, ao contrário, JÁ tinham
// aparecido nos probes originais mas foram derrubados por um erro de
// transcrição ao escrever esta constante -- só pegos rodando de verdade
// DENTRO do nsjail; ver o comentário dedicado deles mais abaixo pro relato
// completo. `execve` também nunca apareceu como uma
// chamada que O PRÓPRIO ruby faz (não lança nenhum processo filho, ao
// contrário do Java/LaunchingConnector) -- mantido mesmo assim, mesma
// razão defensiva de sempre (é o próprio nsjail que precisa exec'ar o
// binário `ruby` depois de instalar o filtro seccomp).
//
// Diferença estrutural em relação às duas políticas anteriores: NÃO há
// nenhum socket de coordenação interno (JDWP pro Java via loopback TCP,
// socket de diagnóstico do CoreCLR via Unix domain) -- TracePoint é
// introspecção in-process, sem protocolo de rede nem IPC de handshake
// nenhum. `socket`/`connect`/etc. abaixo existem só pela MESMA razão já
// documentada nas duas políticas anteriores: deixar uma tentativa de rede
// do PRÓPRIO código do usuário (`TCPSocket.new`/`TCPServer.new`, ambos
// testados de verdade nos probes acima) falhar de forma limpa e
// capturável (isolamento de namespace de rede, não seccomp, é a camada que
// deve bloquear alcance de rede real) em vez de morrer com um SIGSYS
// incapturável -- `accept`/`shutdown`/`socketpair` são a única exceção
// (não exercitados pelos probes acima, incluídos defensivamente pela mesma
// razão de "próximo passo óbvio do ciclo de vida de socket" que
// CSHARP_SECCOMP_POLICY/JAVA_SECCOMP_POLICY já usam pros seus próprios
// syscalls não-exercitados-mas-óbvios).
const RUBY_SECCOMP_POLICY: &str = r#"ALLOW {
    // Ciclo de vida de processo. Ver o comentário acima sobre a ausência
    // real (confirmada, não suposta) de clone/wait4 -- o MRI aqui não usa
    // uma 2ª thread real pro seu próprio mecanismo interno de interrupção
    // (isso não quer dizer que ele nunca usa futex -- ver o comentário
    // dedicado logo abaixo).
    execve, exit_group,
    set_tid_address, set_robust_list, rseq, prctl,
    gettid, getpid, geteuid, getegid, getuid, getgid,
    prlimit64,
    // futex, ppoll: achado real de um erro de TRANSCRIÇÃO, pego só ao
    // rodar de verdade DENTRO do nsjail -- não um gap de derivação. As duas
    // syscalls JÁ estavam na união dos 9 probes originais (`futex` aparece
    // em `s_net.txt`, `ppoll` em `s_net.txt` e `s_misc.txt`, confirmado
    // reconferindo os arquivos de strace salvos), mas foram omitidas por
    // engano ao escrever esta constante à mão pela primeira vez (a
    // reorganização em grupos comentados, feita depois de já ter a lista
    // bruta, perdeu as duas no meio do processo). O EFEITO de rodar dentro
    // do nsjail de verdade (não só stracear fora dele) é exatamente pra
    // pegar esse tipo de erro: `NetworkEscape.rb`
    // (`TCPSocket.new("example.com", 80)`) matou o processo com SIGSYS
    // dentro do jail -- `dmesg`/auditd dentro do container mostrou duas
    // violações em sequência conforme cada uma foi corrigida e revalidada
    // (`sig=31 ... syscall=98` = futex primeiro, depois `syscall=73` =
    // ppoll na tabela aarch64), confirmando via evidência real, não
    // suposição, e batendo exatamente com o que os probes originais já
    // tinham mostrado. Ambas fazem sentido no caminho de resolução de nome
    // que `TCPSocket.new` exercita (glibc/`getaddrinfo` usa lock interno e
    // I/O com timeout pra consultar `/etc/resolv.conf`/DNS). Revalidado
    // rodando o mesmo `NetworkEscape.rb` de novo dentro do nsjail depois
    // das duas correções (ver tasks.md pro resultado real, limpo).
    futex, ppoll,

    // Memória: heap do MRI (objetos, GC generacional), pilha, mmaps de
    // inicialização do interpretador.
    brk, mmap, munmap, mprotect,

    // I/O de arquivo: carregar a stdlib Ruby embutida na imagem (json,
    // objspace, fileutils, socket, e o que mais a stdlib carregar
    // internamente no boot do interpretador), ler driver.rb e o script do
    // usuário, stdout/stderr do próprio driver (write E writev -- MRI usa
    // os dois dependendo do caminho de I/O, confirmado via strace, não só
    // um dos dois como seria de esperar por analogia com Java). Mesma
    // camada de responsabilidade já documentada em
    // JAVA_SECCOMP_POLICY/CSHARP_SECCOMP_POLICY: o chroot read-only é quem
    // deve bloquear acesso indevido a caminho, não esta lista --
    // confirmado com um probe real de `File.read("/etc/shadow")` (roda
    // fora do jail nesta fase de derivação, então lê de verdade -- o que
    // importa aqui é só confirmar QUE syscalls são usados, a fase de
    // validação real dentro do nsjail é o que confirma que o chroot
    // efetivamente bloqueia).
    openat, read, write, writev, close, lseek,
    newfstatat, faccessat, readlinkat,
    getdents64, getcwd, ioctl, fcntl,
    // Mesmo grupo de operações de filesystem "mutantes" já adicionado nas
    // duas políticas anteriores (achado do pentest da Fase 2 -- ver
    // JAVA_SECCOMP_POLICY) -- confirmado de novo aqui via probe real
    // (File.symlink/.rename/.chmod/.utime/.link, `require 'fileutils'`):
    // symlinkat/renameat/fchmodat/utimensat/linkat mapeiam exatamente
    // igual em cima de glibc, independente da linguagem/runtime por cima.
    symlinkat, linkat, renameat, fchmodat, utimensat,

    // Sockets: ver o comentário de módulo acima. sendto especificamente é
    // outro achado real de dentro do jail (3ª rodada de SIGSYS ao validar
    // `NetworkEscape.rb`, depois de futex/ppoll): dmesg/auditd mostrou
    // `syscall=206` (sendto). Diferença de comportamento vs. o probe
    // original fora do jail (que usou `sendmmsg`, já coberto, pra fazer a
    // consulta DNS): dentro do jail `/etc/resolv.conf` está vazio (nenhum
    // nameserver configurado -- ver o bloco de /etc sintético abaixo),
    // então o resolver do glibc cai num caminho de fallback diferente do
    // que usou fora do jail (onde havia um resolver real configurado),
    // gerando uma chamada sendto de um único datagrama em vez do sendmmsg
    // em lote do caminho paralelo A+AAAA. sendmsg/recvmsg incluídos junto,
    // defensivamente, mesma família de syscall/mesma razão -- variantes
    // igualmente plausíveis do mesmo code path que só uma tentativa de
    // conexão de rede real dispara, exatamente o tipo de coisa que só
    // aparece rodando o programa de verdade, não em toda execução.
    socket, connect, bind, listen, accept,
    getsockname, setsockopt, shutdown, socketpair,
    recvfrom, sendto, sendmsg, recvmsg, sendmmsg,

    // Tempo/agendamento/diversos que o MRI usa no boot e durante a
    // execução. getrandom cobre Kernel#rand/SecureRandom -- MRI não abre
    // /dev/urandom diretamente (diferente do CoreCLR, ver
    // CSHARP_SECCOMP_POLICY), confirmado pela ausência de openat sobre
    // esse caminho em qualquer probe.
    clock_gettime, sched_getaffinity, sysinfo, getrandom,
    // eventfd2: usado pelo próprio mecanismo de timer/interrupção do MRI
    // (visto em TODOS os 9 probes, incluindo o mais trivial) -- não é
    // socket nem timer em si, é o descritor de notificação que o MRI cria
    // pra isso.
    eventfd2,
    // newuname, não `uname` -- mesmo achado já documentado em
    // JAVA_SECCOMP_POLICY (tabela aarch64 do kafel nomeia sys_newuname;
    // strace mostra o nome amigável `uname`, mas kafel rejeita esse
    // identificador com "Undefined identifier").
    newuname,
    // timer_create/timer_settime: o mecanismo de interrupção do MRI citado
    // acima -- POSIX timer entregue como sinal, confirmado via strace em
    // todo probe (inclusive os mais triviais), não threading real.
    timer_create, timer_settime,

    // Sinais: MRI instala handlers pro sinal do timer acima e pros sinais
    // de shutdown usuais. rt_sigreturn confirmado especificamente no probe
    // de loop infinito (OutputFlood-equivalente) -- é onde o sinal do
    // timer de fato interrompe a execução em andamento, o que só se
    // observa num programa rodando tempo suficiente pra isso acontecer.
    // restart_syscall NÃO foi observado em nenhum probe (nenhum bloqueou
    // tempo suficiente pra precisar de auto-restart de uma syscall
    // interrompida pelo sinal do timer) -- incluído mesmo assim,
    // defensivamente, pela mesma razão que JAVA_SECCOMP_POLICY/
    // CSHARP_SECCOMP_POLICY já incluem: o MESMO mecanismo de timer por
    // sinal que já confirmamos que existe aqui é exatamente o tipo de
    // coisa que PODE interromper uma syscall bloqueante em algum programa
    // real não coberto pelos 9 probes (ex.: um `sleep` mais longo, ou I/O
    // bloqueante de verdade).
    rt_sigaction, rt_sigprocmask, rt_sigreturn, restart_syscall,
    // sigaltstack: MRI (assim como CoreCLR -- ver CSHARP_SECCOMP_POLICY)
    // usa uma pilha de sinal alternativa para detectar SystemStackError
    // via handler de SIGSEGV na guard page -- é o que permite o
    // `rescue SystemStackError` de driver.rb funcionar de verdade em vez
    // de crashar com um SIGSEGV incapturável. Confirmado via probe real de
    // overflow de pilha (stackoverflow.rb): o rescue disparou e emitiu
    // {"type":"stack_overflow"} corretamente com esta syscall permitida.
    sigaltstack
} DEFAULT KILL"#;

// Mesma ressalva de arquitetura já documentada em JAVA_SECCOMP_POLICY/
// CSHARP_SECCOMP_POLICY: syscalls derivados em linux/arm64 (Apple Silicon
// Docker Desktop), não revalidados em amd64.

// Rootfs mínimo isolado pro Ruby -- ver build-minimal-rootfs.sh e a mesma
// MINIMAL_ROOTFS_JAVA/MINIMAL_ROOTFS_CSHARP em java.rs/csharp.rs.
const MINIMAL_ROOTFS_RUBY: &str = "/opt/sandbox-rootfs/ruby";

// Mesmo mecanismo/motivo do JAIL_WORKDIR de java.rs/csharp.rs: caminho FIXO
// pré-criado em build-minimal-rootfs.sh, porque o workdir real (dinâmico,
// por execução) não pode ser bind-mountado no próprio caminho -- nsjail
// monta a raiz do chroot como somente-leitura ANTES de processar
// --bindmount_ro, então um alvo de mount que ainda não existe dentro do
// rootfs mínimo falha com EACCES.
const JAIL_WORKDIR: &str = "/workdir";

// Caminho fixo do driver dentro do rootfs mínimo (ver build-minimal-rootfs.sh
// -- copiado de /app/ruby-driver.rb, staged no Dockerfile a partir de
// sandbox/ruby/driver.rb). Não precisa de nenhuma etapa de build/CDS -- Ruby
// não compila, então isso é só um arquivo-fonte copiado como está.
const RUBY_DRIVER_PATH: &str = "/app/ruby-driver.rb";

pub struct RunOptions {
    pub time_limit_secs: String,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            // RUBY_TIME_LIMIT_SECS, env var própria (mesmo padrão de
            // JAVA_TIME_LIMIT_SECS/CSHARP_TIME_LIMIT_SECS). Medido de
            // verdade (dentro do container real, `time /app/sandbox-runner
            // --language ruby ...`, mesmo par de programas usado nas
            // medições de Java/C#): um programa moderadamente complexo
            // (iteração aninhada via `each_with_index`, array de 60
            // elementos, chamada de método auxiliar, interpolação de string
            // no caminho quente) bateu o cap de 5.000 eventos em ~0.80s; um
            // loop plano trivial de 20k iterações (equivalente ao
            // BigCountLoop.java/BigCountLoop C#) bateu o MESMO cap em
            // ~0.79s -- praticamente idêntico, ao contrário de Java/C#
            // (onde a complexidade dos locals/pilha por passo domina o
            // tempo) porque aqui é tudo in-process: sem round-trip JDWP
            // nem ICorDebug, só o custo do próprio callback do TracePoint +
            // serialização json. Isso é 1-2 ordens de grandeza mais rápido
            // que os piores casos medidos de Java (~8.9s) e C# (~3.5s) --
            // esperado dado que não há 2º processo nem protocolo remoto
            // envolvido. 10s (bem menor que os 20-25s das outras duas
            // linguagens) já dá ~12x de margem sobre o pior caso medido
            // aqui, ainda generoso o bastante pra variância real de carga
            // do host sem carregar um timeout desproporcional ao overhead
            // real observado.
            time_limit_secs: env::var("RUBY_TIME_LIMIT_SECS").unwrap_or_else(|_| "10".into()),
        }
    }
}

/// Roda um .rb isolado via nsjail, com driver.rb instrumentando via
/// TracePoint. Eventos JSON saem no stdout do processo atual (herdado do
/// child, via events::run_nsjail). Sem etapa de compilação -- ao contrário
/// de java.rs (javac) e do lado C# de ProcessSandboxRunner (dotnet build),
/// Ruby não precisa de nada rodando antes do nsjail além de garantir que o
/// workdir é legível pelo uid não-privilegiado (mesma razão de sempre, ver
/// make_world_readable).
pub fn run(ruby_file: &Path, opts: &RunOptions) -> std::process::ExitStatus {
    let file_name = ruby_file
        .file_name()
        .and_then(|s| s.to_str())
        .expect("nome de arquivo inválido");

    let src_dir = ruby_file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    // Mesma razão de sempre (ver java.rs/csharp.rs): ProcessSandboxRunner
    // (API, rodando como root) cria o workdir via Files.createTempDirectory,
    // que fica 0700 independente de umask -- ilegível pelo uid não-root que
    // nsjail mapeia o processo jailed para.
    events::make_world_readable(src_dir).expect("falha ao ajustar permissões do workdir");

    eprintln!("[sandbox-runner/ruby] rodando {file_name} isolado via nsjail...");

    let mut cmd = Command::new("nsjail");
    cmd.args([
        "--mode", "o",
        "--time_limit", &opts.time_limit_secs,
        "--rlimit_as", "1024",
        "--rlimit_cpu", &opts.time_limit_secs,
        "--rlimit_nproc", "128",
        "--rlimit_nofile", "256",
        "--use_cgroupv2",
        // Bem menor que os 512MB do Java/256MB do C# -- MRI não tem heap
        // JIT/metaspace/compressed-class-space pra reservar, e este driver
        // não lança um 2º processo (ao contrário do Java, que soma
        // driver+alvo no mesmo cgroup). 128MB validado como suficiente
        // pros test-snippets-ruby/ reais durante esta tarefa -- ver
        // tasks.md se isso precisar subir no futuro (ex.: um programa que
        // realmente aloca muita coisa de propósito).
        "--cgroup_mem_max", "134217728",
        // memory.swap.max=0 -- mesma razão/verificação já documentada em
        // java.rs/csharp.rs (força SIGKILL imediato em vez de thrashing de
        // swap do HOST).
        "--cgroup_mem_swap_max", "0",
        "--cgroup_pids_max", "128",
        // Não-root dentro do jail -- mesma razão/mecanismo (--uid_mapping/
        // --gid_mapping, não --user/--group) já documentada em java.rs.
        "--uid_mapping", "65534:65534:1",
        "--gid_mapping", "65534:65534:1",
        "--seccomp_string", RUBY_SECCOMP_POLICY,
        "--chroot", MINIMAL_ROOTFS_RUBY,
        "--bindmount_ro", &format!("{}:{}", src_dir.to_str().unwrap(), JAIL_WORKDIR),
        "--cwd", JAIL_WORKDIR,
        // NÃO --quiet -- mesma razão de sempre: precisamos do log INFO do
        // próprio nsjail ("run time >= time limit") pra distinguir um kill
        // por --time_limit de um OOM kill de cgroup (events::run_nsjail).
        "--",
        "/usr/bin/ruby",
        RUBY_DRIVER_PATH,
        file_name,
    ]);

    let result = events::run_nsjail(cmd);
    match result.outcome {
        RunOutcome::TimedOut => events::emit(&Event::Timeout),
        RunOutcome::LikelyOom => events::emit(&Event::MemoryLimitExceeded),
        RunOutcome::OutputTruncated => events::emit(&Event::OutputTruncated),
        RunOutcome::Normal => {
            // Defesa em profundidade: driver.rb já captura SystemStackError
            // in-process e emite {"type":"stack_overflow"} limpo por conta
            // própria ANTES de qualquer possibilidade de crash cru (ver o
            // comentário desse rescue em driver.rb sobre a margem de pilha
            // reservada pelo MRI especificamente pra isso funcionar) -- ao
            // contrário de Java/C#, que dependem só de um marcador de texto
            // no stderr porque o driver está NUM PROCESSO SEPARADO do que
            // estourou a pilha. Este marcador aqui só existiria como rede
            // de segurança caso o rescue in-process falhe por algum motivo
            // não previsto (ex.: estouro tão abrupto que nem a margem
            // reservada do MRI é suficiente) -- "stack level too deep" é a
            // mensagem real que o MRI usa pra SystemStackError#message.
            if result.stderr_lines.iter().any(|l| l.contains("stack level too deep")) {
                events::emit(&Event::StackOverflow);
            }
        }
    }
    result.status
}
