// Runtime C# (ICorDebug via interop direto — ver com.rs). Diferente do Java
// (onde nsjail lança um processo Java separado que fala JDWP com o driver),
// aqui a lógica de debug roda DENTRO do nosso próprio binário Rust — porque
// dbgshim precisa manipular o processo debuggee diretamente via FFI.
//
// Por isso o padrão é "self re-exec": o processo externo (run_outer) faz
// fork+exec do nsjail apontando pra ELE MESMO de novo, mas com uma flag
// interna (--csharp-worker) que faz o binário, já dentro do jail, pular
// direto pra run_worker() em vez de tentar reabrir o nsjail recursivamente.
//
// Flags de nsjail já validadas empiricamente no spike (não inventar de novo):
//   --rlimit_fsize inf       (CoreCLR faz memfd_create("doublemapper") +
//                              ftruncate pra 2TB — reserva virtual, não disco
//                              real; sem isso o processo morre com SIGXFSZ)
//   --tmpfsmount /tmp        (o handshake de debug do CoreCLR cria socket/pipes
//                              em /tmp — se for read-only, EROFS e trava)
//   DOTNET_GCHeapHardLimit   (sem isso o CoreCLR tenta dimensionar o heap pela
//                              RAM total do host e estoura o rlimit_as)

use std::os::raw::{c_int, c_void};
use std::path::Path;
use std::process::Command;
use std::ptr;
use std::time::{Duration, Instant};

use libloading::Library;

use crate::com;
use crate::events::{self, Event, RunOutcome};
use crate::pdb::PortablePdb;

pub const CSHARP_WORKER_FLAG: &str = "--csharp-worker";

// Fase 2 hardening: path to the minimal, isolated rootfs `nsjail --chroot`s
// into instead of the container's own "/" — see java.rs's identical
// MINIMAL_ROOTFS_JAVA constant and sandbox/build-minimal-rootfs.sh's header
// comment (built at image-build time by that same script, into
// /opt/sandbox-rootfs/csharp this time). Same tasks.md item: "Filesystem
// temporário/efêmero".
const MINIMAL_ROOTFS_CSHARP: &str = "/opt/sandbox-rootfs/csharp";

// Fixed internal path the real (per-execution, dynamically-named) workdir
// gets bind-mounted onto inside the jail -- see java.rs's identical constant
// and its comment at the --bindmount_ro call site in `run_outer` for why.
const JAIL_WORKDIR: &str = "/workdir";

// Fase 2 hardening: minimal default-deny seccomp-bpf allowlist for the
// jailed process -- see java.rs's JAVA_SECCOMP_POLICY doc comment for the
// kafel syntax verification (same nsjail/kafel source, same `--help`
// check), shared here rather than repeated. Key difference from Java:
// there's no separate "javac equivalent" running outside the jail here --
// `dotnet build` happens on the API side (ProcessSandboxRunner, outside
// sandbox-runner entirely) before sandbox-runner is even invoked, so
// everything THIS binary does once re-exec'd into the jail (--csharp-worker,
// see run_worker below) -- dlopen'ing libdbgshim.so, the dbgshim/ICorDebug
// handshake, AND launching+debugging the actual `dotnet <dll>` debuggee
// child process -- has to be covered by this one policy.
//
// Derived the same way as Java's: UNION of `strace -f` runs of the exact
// `/app/sandbox-runner --csharp-worker --dll <dll>` command line (same
// self-re-exec this file performs below, run directly instead of via
// nsjail) across every project in `test-snippets-csharp/`, PLUS two
// throwaway probes (not committed test-snippets -- the official C# suite
// has no equivalent of Java's NetworkEscape.java/FilesystemEscape.java)
// mirroring those two Java snippets, to check the same escape-attempt path
// empirically instead of leaving it silently unvalidated. Real finding from
// that probe, not a guess: a raw `Socket.Connect` in C# already gets
// blocked by THIS sandbox's own pre-existing multi-thread guard (>2
// distinct ICorDebug threads = blocked, MVP scope, see com.rs) before ever
// reaching a real connect() syscall -- .NET's socket layer lazily spins up
// an internal epoll-readiness thread on first use, which trips that guard
// first. So unlike Java, raw sockets in C# are already a dead end today for
// a reason entirely unrelated to seccomp, and this policy does NOT need
// connect/accept/etc. -- omitting them doesn't introduce any new crash
// class, the attempt already ends in a caught "error" event either way.
// (The filesystem half of that probe -- File.ReadAllText("/etc/shadow"),
// File.WriteAllText outside cwd -- needed nothing beyond what the official
// suite already exercises.) Same arm64-derivation caveat as java.rs's
// JAVA_SECCOMP_POLICY applies here too -- not verified on amd64.
const CSHARP_SECCOMP_POLICY: &str = r#"ALLOW {
    // Process/thread lifecycle. execve is the dbgshim `CreateProcessForLaunch`
    // call actually launching `/usr/share/dotnet/dotnet <dll>` as a real
    // (suspended, then resumed) child process. clone covers CoreCLR's
    // internal threads (GC, thread pool, finalizer) and this worker
    // process's own threads.
    execve, clone, exit, exit_group, wait4,
    set_tid_address, set_robust_list, rseq, prctl,
    gettid, getpid, geteuid, getsid,
    // futex: NOT optional -- CoreCLR's GC/thread-pool/finalizer threads and
    // this worker's own ICorDebug callback synchronization all depend on
    // it, even for a single-user-thread program. ppoll is used waiting on
    // the dbgshim/debug-pipe file descriptors. prlimit64 is CoreCLR reading
    // its own rlimits at startup (RLIMIT_AS/RLIMIT_NOFILE etc., the ones
    // this same nsjail invocation sets) to size the GC/thread pool.
    futex, ppoll, prlimit64,

    // Memory management. memfd_create + msync are CoreCLR's GC "double
    // mapper" (see this file's own module doc comment above -- a memfd
    // reserved up to ~2TB of virtual address space, not real disk/RAM;
    // --rlimit_fsize inf next to this same nsjail invocation exists for the
    // same reason). get_mempolicy/sched_setaffinity/sched_getaffinity are
    // the GC probing NUMA/CPU topology to size itself -- confirmed real,
    // not assumed, since they show up even for MemoryHog's trivial single
    // allocation loop.
    brk, mmap, munmap, mprotect, madvise, memfd_create, msync,
    get_mempolicy,

    // File I/O: dlopen'ing libdbgshim.so, reading the target assembly
    // (<dll>) plus its dependent CoreCLR assemblies under
    // /usr/share/dotnet, the worker's own stdout/stderr, and (see
    // this file's module doc comment) the debug-session handshake pipes
    // dbgshim creates under /tmp (mknodat for the named FIFOs, linkat/
    // fchmod as part of that same setup, chdir into the dll's own
    // directory as cwd). Same layering note as Java's identical comment:
    // FilesystemEscape-style attempts (confirmed via a throwaway probe --
    // see this const's doc comment) use nothing beyond this same list.
    openat, read, write, close, pread64, lseek,
    // newfstat, not `fstat` -- see JAVA_SECCOMP_POLICY's identical comment
    // in java.rs for the full story (kafel names the raw fstat(2)-on-an-
    // open-fd syscall `newfstat` on aarch64, confirmed both by kafel's
    // generated table AND by directly stracing this exact nsjail+seccomp
    // invocation, which showed CoreCLR itself calling real `fstat(fd, ...)`
    // repeatedly while loading its own assemblies).
    newfstat, newfstatat, statx, statfs, faccessat, readlinkat,
    getdents64, unlinkat, ftruncate, linkat, fchmod, mknodat, chdir,
    ioctl, fcntl, pipe2,
    // Real gap found during the Fase 2 pentest (tasks.md "Testes de fuga de
    // sandbox"), C# side of the SAME fix already applied to
    // JAVA_SECCOMP_POLICY -- see that constant's identical comment in
    // java.rs for the full empirical trail (strace showing the exact
    // syscalls, the A/B control confirming a missing syscall here means an
    // uncatchable SIGSYS instead of the intended clean, catchable
    // filesystem exception). Confirmed via the same throwaway-probe
    // methodology as this const's own doc comment above (not the official
    // test-snippets-csharp/ suite, which happens to call none of these):
    // `System.IO.File.CreateSymbolicLink`/`Move`/`SetUnixFileMode` map to
    // symlinkat/renameat/fchmodat respectively (`linkat`/`fchmod` were
    // already allowed above, for dbgshim's own unrelated setup) --
    // `utimensat` (File.SetLastWriteTimeUtc) added alongside for the same
    // reason, confirmed via its own isolated strace. NOTE: unlike the Java
    // case, the end-to-end symptom here was masked by the ALREADY-DOCUMENTED
    // open item on this stepper getting stuck on certain CLR-internal
    // transitions (see tasks.md, the StartCore/exception-unwind items) --
    // calling any of these still timed out rather than turning into a clean
    // in-product exception either way, so this fix is defense-in-depth /
    // consistency with the stated design principle (chroot, not seccomp, is
    // supposed to gate which paths succeed), not something fully validated
    // end-to-end the way the Java fix was — see tasks.md for the honest
    // writeup of what this did and did not confirm.
    symlinkat, renameat, fchmodat, utimensat,

    // Sockets: CoreCLR opens a local Unix-domain diagnostics IPC socket
    // (`/tmp/dotnet-diagnostic-<pid>-...-socket`) at startup by default,
    // regardless of whether anything ever connects to it -- confirmed via
    // strace (socket/bind/listen), not assumed. connect/accept/etc. are
    // deliberately NOT here -- see this const's doc comment on why real
    // outbound sockets are already a dead end in C# today for an unrelated
    // reason (the multi-thread guard), so they're not "needed to run
    // correctly" by the definition this policy uses.
    socket, bind, listen,

    // Time/scheduling/misc CoreCLR needs at startup and for its thread
    // pool/GC (clock_gettime itself is NOT here -- see this const's doc
    // comment: unlike Java, every trace showed it fully resolved via vDSO,
    // no real syscall trap; kept out deliberately rather than added
    // speculatively, per this project's "don't assume, validate
    // empirically" rule -- if a future run ever needs the real syscall
    // fallback, that will surface as a SIGSYS in end-to-end validation, not
    // silently).
    clock_nanosleep, sched_yield, sched_getaffinity, sched_setaffinity,
    sched_getparam, sched_getscheduler, sched_setscheduler,
    sched_get_priority_max, sched_get_priority_min,
    sysinfo, getrandom, membarrier,

    // Signals: sigaltstack backs CoreCLR's own SIGSEGV-based stack-overflow
    // detection (StackOverflowCs -- see this file's run_outer doc comment
    // on the "Stack overflow." stderr marker), on top of the same
    // rt_sigaction/rt_sigprocmask/rt_sigreturn set Java needs.
    sigaltstack, rt_sigaction, rt_sigprocmask, rt_sigreturn, restart_syscall,

    // epoll: .NET's socket/thread-pool readiness engine initializes lazily
    // on first async use (observed via the throwaway network probe, see
    // this const's doc comment) -- kept since other legitimate async
    // paths (Task, Timer, thread-pool work items) can trigger the same
    // lazy init even without raw sockets.
    epoll_create1, epoll_pwait
} DEFAULT KILL"#;

/// Caminho fixo: única cópia de libdbgshim.so na imagem, empacotada junto
/// com o netcoredbg (confirmado via `find / -iname libdbgshim*` na imagem
/// sandbox-spike) — o SDK do .NET puro não inclui essa lib.
const LIBDBGSHIM_PATH: &str = "/usr/share/netcoredbg/netcoredbg/libdbgshim.so";

type HResult = i32;
type Handle = *mut c_void;
type DWord = u32;
type WChar = u16;

type CreateProcessForLaunchFn = unsafe extern "C" fn(
    *mut WChar,
    c_int,
    *mut c_void,
    *const WChar,
    *mut DWord,
    *mut Handle,
) -> HResult;
type ResumeProcessFn = unsafe extern "C" fn(Handle) -> HResult;
type StartupCallback = extern "C" fn(*mut c_void, *mut c_void, HResult);
type RegisterForRuntimeStartupFn =
    unsafe extern "C" fn(DWord, StartupCallback, *mut c_void, *mut *mut c_void) -> HResult;

static mut CALLBACK_FIRED: bool = false;
static mut CALLBACK_HR: HResult = 0;
static mut P_CORDB: *mut c_void = ptr::null_mut();
// Loaded once up front in run_worker (single dll/execution per process —
// same lifetime assumption as com.rs's other statics), then read from
// com::LOCAL_NAME_RESOLVER's plain-fn callback (no captured state allowed,
// see the comment on LOCAL_NAME_RESOLVER in com.rs), hence a static instead
// of a local variable threaded through.
static mut PDB: Option<PortablePdb> = None;
// Set once at the top of run_worker, read from the STEP_SINK closure below
// to fill Event::Step's time_ns — same reason as PDB above: STEP_SINK is a
// plain `fn` pointer (com::STEP_SINK's declared type), so it can't capture
// a local `t0` the way jdi/Debugger.java's t0 is a plain local variable.
// Mirrors Java's "elapsed since the driver started" semantics.
static mut RUN_START: Option<Instant> = None;

extern "C" fn startup_callback(p_cordb: *mut c_void, _parameter: *mut c_void, hr: HResult) {
    unsafe {
        CALLBACK_FIRED = true;
        CALLBACK_HR = hr;
        P_CORDB = p_cordb;
    }
}

fn to_utf16(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

fn emit_error(message: impl Into<String>) -> i32 {
    let message = message.into();
    eprintln!("[sandbox-runner/csharp/worker] erro: {message}");
    events::emit(&Event::Error { message });
    1
}

pub struct RunOptions {
    pub time_limit_secs: String,
    // Sampling rate knob, C#-side port of java.rs's identical field (see
    // that struct's doc comment) — same env var name (SPIKE_SAMPLE) on
    // purpose, for cross-language consistency; see run_outer's doc comment
    // on why it's forwarded explicitly via `--env` rather than relied on to
    // pass through nsjail implicitly.
    pub sample_n: String,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            // Renamed from the spike-era SPIKE_TIME_LIMIT — see java.rs's
            // identical RunOptions::default comment for the full rationale
            // (production-sounding name, split per-language env var,
            // trace-and-replay single-wall-clock model, 5,000-step cap
            // interaction). Kept a SEPARATE env var from Java's
            // (CSHARP_TIME_LIMIT_SECS, not a shared name) because the two
            // runtimes have genuinely different measured per-step costs —
            // see below.
            //
            // Default 15s (unchanged numerically from the old spike default,
            // but now a deliberately DERIVED value, not a leftover). Real
            // measurement (docker run, --privileged --cgroupns=host, this
            // exact image): a moderately complex program (nested loops, a
            // 60-element array, a helper method call, string concatenation
            // in the hot path — same shape of program used for the Java
            // measurement, for a fair comparison) hit the 5,000-event cap in
            // ~3.5s wall time; a trivial flat 20k-iteration loop
            // (BigCountLoop) hit it in ~2.0s. Both comfortably faster than
            // Java's equivalents (~8.9s / ~5.3s) — consistent with the
            // "Throttling/amostragem" finding already in tasks.md that C#'s
            // dominant per-step cost is the ICorDebug round-trip itself, not
            // locals/stack extraction, and with the observed stack traces
            // here actually descending into CLR-internal frames (string
            // formatting helpers, Memmove, etc.) that JDI's coarser
            // STEP_OVER never surfaces for the Java equivalent. 15s gives
            // ~4.3x margin over the ~3.5s worst case measured — a larger
            // margin ratio than Java's ~2.8x, deliberately: C# has a real,
            // documented stepper-hang flakiness unrelated to the step cap
            // (tasks.md "stepper do ICorDebug trava indefinidamente" in
            // StartCore/exception-unwind cases) where an otherwise-legitimate
            // run can genuinely need to wait out most of the budget before
            // nsjail's own --time_limit reaps it — extra headroom here isn't
            // just paranoia, it's covering a known correctness gap this task
            // does not fix.
            time_limit_secs: std::env::var("CSHARP_TIME_LIMIT_SECS").unwrap_or_else(|_| "15".into()),
            sample_n: std::env::var("SPIKE_SAMPLE").unwrap_or_else(|_| "1".into()),
        }
    }
}

/// Chamado pelo dispatcher normal (fora do jail): compila o projeto e faz
/// fork+exec do nsjail apontando pro nosso próprio binário com
/// `--csharp-worker`, que vai rodar run_worker() já isolado.
///
/// Fase 2 hardening: run via `events::run_nsjail` instead of a bare
/// `.status()` with everything inherited, so a `--time_limit` kill or a
/// cgroup OOM kill of the worker (which run_worker() itself has no chance
/// to react to — it just dies) still produces a clean `timeout`/
/// `memory_limit_exceeded` event on our stdout. CoreCLR's own stack
/// overflow behavior was tested empirically (see tasks.md "Fase 2"):
/// unlike Java's `java.lang.StackOverflowError`, CoreCLR prints a distinct
/// `Stack overflow.` line to stderr (with a repeated-frame summary) and
/// exits cleanly with status 1 — NOT a raw SIGSEGV/SIGABRT crash as
/// initially assumed before testing. Same detection shape as Java's
/// StackOverflowError marker, just a different literal string.
pub fn run_outer(dll_file: &Path, opts: &RunOptions) -> std::process::ExitStatus {
    let cwd = dll_file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let self_exe = std::env::current_exe().expect("não achou o próprio binário");

    // Same reason as java.rs's identical call: ProcessSandboxRunner (API,
    // running as root) creates the workdir via Files.createTempDirectory
    // (0700 regardless of umask) — unreadable by the non-root uid nsjail
    // maps the jailed process to below. See make_world_readable's doc
    // comment. IMPORTANT: chmod from the workdir itself (cwd's PARENT, e.g.
    // `.../code2complexity-<uuid>`), not just `cwd` (`.../out`) — chdir()
    // needs traverse permission on every path component up to `--cwd`, and
    // ProcessSandboxRunner.compileCsharp puts the dll in a `out/`
    // subdirectory of the 0700 workdir, so fixing only `out/` would still
    // leave the workdir itself blocking traversal into it.
    let workdir = cwd.parent().unwrap_or(cwd);
    events::make_world_readable(workdir).expect("falha ao ajustar permissões do workdir");

    // Fase 2 hardening: `workdir` gets bind-mounted onto the FIXED
    // JAIL_WORKDIR path below (not its own real, per-execution path) --
    // see java.rs's identical JAIL_WORKDIR comment for why (mounting onto a
    // dynamic path that doesn't already exist inside the minimal rootfs
    // fails: nsjail's own mkdir-the-missing-mountpoint step runs AFTER the
    // chroot root is already read-only). `cwd` and `dll_file` are `workdir`-
    // relative, so their JAIL-side equivalents are derived the same way,
    // rather than hardcoding the "out/" subdir name ProcessSandboxRunner
    // happens to use today.
    let cwd_suffix = cwd.strip_prefix(workdir).unwrap_or_else(|_| Path::new(""));
    let jail_cwd = Path::new(JAIL_WORKDIR).join(cwd_suffix);
    let jail_dll = jail_cwd.join(dll_file.file_name().expect("dll_file sem nome de arquivo"));

    eprintln!("[sandbox-runner/csharp] rodando {dll_file:?} isolado via nsjail (self re-exec)...");

    // Hoisted out instead of an inline `&format!(...)` in the args array
    // below (the pattern java.rs and this file's other formatted nsjail
    // args use) purely so the long "--env" comment has somewhere to attach
    // without cluttering the array literal — a temporary inline would have
    // lived long enough either way (extended to the end of the `cmd.args()`
    // statement).
    let sample_env = format!("SPIKE_SAMPLE={}", opts.sample_n);

    let mut cmd = Command::new("nsjail");
    cmd.args([
        "--mode", "o",
        "--time_limit", &opts.time_limit_secs,
        "--rlimit_fsize", "inf",
        "--tmpfsmount", "/tmp",
        // Fase 2 hardening: found empirically while validating the minimal
        // rootfs (see MINIMAL_ROOTFS_CSHARP/tasks.md "Filesystem temporário/
        // efêmero") that CoreCLR's PAL layer needs a REAL /dev/shm for the
        // debugger-attach handshake specifically: it opens POSIX named
        // semaphores there (/dev/shm/sem.clrco*/sem.clrst* — "CLR
        // coordination"/"CLR startup", confirmed via strace), which a plain
        // (non-debugged) `dotnet <dll>` run never touches — so this was
        // invisible until RegisterForRuntimeStartup was exercised for real,
        // where it failed with hr=0x80070490 (ERROR_NOT_FOUND) against a
        // rootfs with no /dev/shm at all. A fresh tmpfs (not a bind-mount of
        // the host's own /dev/shm) keeps semaphore names isolated per
        // execution, same isolation property as --tmpfsmount /tmp above.
        "--tmpfsmount", "/dev/shm",
        "--rlimit_as", "3072",
        "--rlimit_cpu", &opts.time_limit_secs,
        "--rlimit_nproc", "256",
        "--rlimit_nofile", "256",
        "--use_cgroupv2",
        "--cgroup_mem_max", "268435456",
        // Fase 2 hardening: memory.swap.max=0 — same rationale/verification
        // as java.rs's identical flag (see the comment there): forces an
        // immediate cgroup OOM kill instead of swap thrashing the host.
        "--cgroup_mem_swap_max", "0",
        "--cgroup_pids_max", "256",
        // Non-root inside the jail — see java.rs's identical flag for the
        // full rationale (why --uid_mapping/--gid_mapping, not the simpler
        // --user/--group). NOTE: --keep_caps above still retains the full
        // capability set (CAP_DAC_OVERRIDE included) regardless of uid, so
        // this alone does not yet give C# the same real privilege
        // separation Java gets — see the TODO on --keep_caps.
        "--uid_mapping", "65534:65534:1",
        "--gid_mapping", "65534:65534:1",
        // Fase 2 hardening: minimal default-deny seccomp-bpf allowlist --
        // see CSHARP_SECCOMP_POLICY's doc comment above for how this
        // syscall set was derived and validated.
        "--seccomp_string", CSHARP_SECCOMP_POLICY,
        // Fase 2 hardening: real isolated rootfs, not the container's own "/"
        // — see MINIMAL_ROOTFS_CSHARP's doc comment and java.rs's identical
        // change (same rationale, same tasks.md item: "Filesystem
        // temporário/efêmero"). `workdir` (not just `cwd`) is bind-mounted at
        // its own real absolute path for the same reason
        // `make_world_readable` above is also called on `workdir`, not
        // `cwd`: the dll/pdb/deps.json/runtimeconfig.json live in `cwd`
        // (workdir's `out/` subdir), but dbgshim's own handshake needs
        // traversal permission through `workdir` itself too. `/dev/urandom`
        // is bound because CoreCLR's crypto RNG opens it directly (confirmed
        // via strace — NOT covered by the `getrandom` syscall alone, unlike
        // Java, which never touches this device — see
        // build-minimal-rootfs.sh's comment on the same finding).
        //
        // Fase 2 pentest finding, fixed (tasks.md "Testes de fuga de
        // sandbox"): this used to also `--bindmount_ro "/sys"`, same
        // informational-only cgroup/topology-read rationale as java.rs's
        // identical bind (already overridden here by the explicit
        // DOTNET_GCHeapHardLimit env below) — and the SAME real information-
        // disclosure bug java.rs's identical removal fixes: binding the
        // host's real /sys pulls in the whole host /sys/fs/cgroup tree
        // (because the OUTER container runs `--cgroupns=host`), letting
        // jailed code list sibling `NSJAIL.<pid>` cgroup names from OTHER
        // concurrent executions and read host-wide aggregate counters like
        // `/sys/fs/cgroup/docker/memory.current`. Not re-derived separately
        // for C# (same underlying mechanism, same fix) — see java.rs's
        // matching comment for the full empirical trail. Removed; the
        // test-snippets-csharp/ suite was re-validated with it gone (see
        // tasks.md) and nothing regressed.
        "--chroot", MINIMAL_ROOTFS_CSHARP,
        "--bindmount_ro", &format!("{}:{}", workdir.to_str().unwrap(), JAIL_WORKDIR),
        "--bindmount_ro", "/dev/urandom",
        "--cwd", jail_cwd.to_str().unwrap(),
        "--env", "DOTNET_ROOT=/usr/share/dotnet",
        "--env", "PATH=/usr/share/dotnet:/usr/bin:/bin",
        "--env", "DOTNET_GCHeapHardLimit=0x8000000",
        // Explicit --env forward, not a bare env::var() read inside
        // run_worker: nsjail does NOT pass the parent's environment through
        // by default — confirmed empirically (a stray first attempt at this
        // that appended `--env SPIKE_SAMPLE=...` AFTER the `--` separator
        // below instead of before it made nsjail try to execve("--env")
        // as the target program itself and fail immediately; once moved
        // before `--`, same place as the other --env flags, it works).
        // Every other var run_worker needs from outside the jail
        // (DOTNET_ROOT, PATH, DOTNET_GCHeapHardLimit above) is already
        // forwarded this same explicit way; SPIKE_SAMPLE follows the
        // identical pattern rather than relying on undocumented passthrough.
        // MUST stay before "--": everything after that separator is nsjail's
        // argv for the jailed program, not more nsjail flags.
        "--env", &sample_env,
        // NOT --quiet — see the identical comment in java.rs::run(): we
        // need nsjail's own INFO-level "run time >= time limit" log line to
        // tell a timeout kill apart from a cgroup OOM kill (both produce
        // the exact same exit code otherwise).
        "--",
    ])
    .arg(self_exe)
    .args([CSHARP_WORKER_FLAG, "--dll", jail_dll.to_str().unwrap()]);

    let result = events::run_nsjail(cmd);
    match result.outcome {
        RunOutcome::TimedOut => events::emit(&Event::Timeout),
        RunOutcome::LikelyOom => events::emit(&Event::MemoryLimitExceeded),
        RunOutcome::OutputTruncated => events::emit(&Event::OutputTruncated),
        RunOutcome::Normal => {
            if result.stderr_lines.iter().any(|l| l.contains("Stack overflow.")) {
                events::emit(&Event::StackOverflow);
            }
        }
    }
    result.status
}

/// Chamado quando o binário já está DENTRO do nsjail (detectado via
/// --csharp-worker no dispatcher). Porta a lógica validada em
/// src/icordebug_spike.rs (attach via dbgshim, ICorDebug, breakpoint no
/// método de entrada, stepping) pro caminho de produção: em vez de parar
/// depois de um número fixo de passos e só imprimir em stderr, cada
/// StepComplete de verdade (ver com.rs::cb_step_complete) emite um
/// `events::Event::Step` via stdout — o modelo de produto é trace-and-replay
/// (grava a execução inteira, sem amostragem aqui). O cap de 5.000 eventos
/// (`events::STEP_EVENT_CAP`) já está implementado em com.rs, mesma decisão
/// de escopo do lado Java.
pub fn run_worker(dll_file: &Path) -> i32 {
    let cwd = dll_file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    // Sinks que os callbacks de com.rs usam pra emitir eventos de verdade
    // (indireção necessária porque com.rs é compartilhado com o binário
    // legado icordebug-spike, que não tem o módulo `events` — ver comentário
    // em com.rs junto de STEP_SINK/ERROR_SINK).
    unsafe {
        RUN_START = Some(Instant::now());
        com::STEP_SINK = Some(|line, locals, stack| {
            let time_ns = RUN_START.map(|t0| t0.elapsed().as_nanos() as u64);
            events::emit(&Event::Step {
                line,
                locals,
                stack,
                time_ns,
                memory_bytes: None,
            });
        });
        com::ERROR_SINK = Some(|message| {
            eprintln!("[sandbox-runner/csharp/worker] erro (callback): {message}");
            events::emit(&Event::Error { message });
        });
        com::LIMIT_SINK = Some(|| {
            events::emit(&Event::StepLimitExceeded);
        });
        com::PROCESS_EXITED = false;
        com::FATAL_ERROR = false;
        com::STEP_EVENTS_EMITTED = 0;
        com::STEP_CAPPED = false;
        com::STEP_EVENTS_TOTAL = 0;
        // Parity port of jdi/Debugger.java's `spike.sample` (see
        // com::SAMPLE_N/STEP_EVENTS_TOTAL and cb_step_complete's doc
        // comment). Same SPIKE_SAMPLE env var name Java already uses, for
        // cross-language consistency (see RunOptions::sample_n's doc
        // comment) — deliberately NOT renamed to something more
        // production-sounding here; see tasks.md for that scope decision.
        // Read directly from the environment (not threaded through as a fn
        // arg) because run_worker is invoked from main.rs's dispatcher with
        // just `--dll`, no RunOptions — same pattern CSHARP_TIME_LIMIT_SECS
        // already uses a few lines below for the inner deadline. run_outer
        // forwards it explicitly via `--env SPIKE_SAMPLE=...` (see that
        // function), so it's guaranteed present here inside the jail.
        com::SAMPLE_N = std::env::var("SPIKE_SAMPLE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1)
            .max(1);

        // PDB is optional: `dotnet build -c Debug` produces one right next
        // to the dll (see ProcessSandboxRunner.compileCsharp on the API
        // side), but if it's missing/unparseable for any reason,
        // LOCAL_NAME_RESOLVER/LINE_RESOLVER stay None and extract_locals/
        // cb_step_complete fall back to the positional "local_N" naming and
        // raw IL offset that always worked before either resolver existed.
        PDB = PortablePdb::load(dll_file);
        com::LOCAL_NAME_RESOLVER = if (*std::ptr::addr_of!(PDB)).is_some() {
            Some(|token, offset| {
                (*std::ptr::addr_of!(PDB)).as_ref().map(|p| p.locals_for(token, offset)).unwrap_or_default()
            })
        } else {
            None
        };
        // Same PDB, same static-fn-pointer indirection as LOCAL_NAME_RESOLVER
        // right above (com::LINE_RESOLVER's own doc comment explains why a
        // plain `fn` is required here) — resolves (method token, IL offset)
        // to a real C# source line via the PDB's SequencePoints blob (see
        // pdb.rs::PortablePdb::line_for). Registered only when a PDB
        // actually loaded, same condition as the locals resolver, since
        // both read from the same `PDB` static.
        com::LINE_RESOLVER = if (*std::ptr::addr_of!(PDB)).is_some() {
            Some(|token, offset| (*std::ptr::addr_of!(PDB)).as_ref().and_then(|p| p.line_for(token, offset)))
        } else {
            None
        };
        // Same PDB/indirection again, this time for line-GRANULAR stepping
        // (see com::STEP_RANGE_RESOLVER's doc comment): resolves (method
        // token, current IL offset) -> the IL range of the covering sequence
        // point, via pdb.rs::PortablePdb::step_range_for. This is what lets
        // com.rs's arm_step call ICorDebugStepper::StepRange instead of
        // plain Step, so StepComplete fires once per SOURCE LINE instead of
        // once per IL instruction — the actual fix for the "same line
        // highlighted N times in a row" disclaimer (see tasks.md). Same
        // None-when-no-PDB condition as the two resolvers above, since it
        // reads from the same PDB static.
        com::STEP_RANGE_RESOLVER = if (*std::ptr::addr_of!(PDB)).is_some() {
            Some(|token, offset| (*std::ptr::addr_of!(PDB)).as_ref().and_then(|p| p.step_range_for(token, offset)))
        } else {
            None
        };
    }

    let lib = match unsafe { Library::new(LIBDBGSHIM_PATH) } {
        Ok(l) => l,
        Err(e) => return emit_error(format!("falha ao carregar libdbgshim.so ({LIBDBGSHIM_PATH}): {e}")),
    };

    let create_process_for_launch: libloading::Symbol<CreateProcessForLaunchFn> =
        match unsafe { lib.get(b"CreateProcessForLaunch\0") } {
            Ok(s) => s,
            Err(e) => return emit_error(format!("símbolo CreateProcessForLaunch não encontrado: {e}")),
        };
    let resume_process: libloading::Symbol<ResumeProcessFn> = match unsafe { lib.get(b"ResumeProcess\0") } {
        Ok(s) => s,
        Err(e) => return emit_error(format!("símbolo ResumeProcess não encontrado: {e}")),
    };
    let register_for_runtime_startup: libloading::Symbol<RegisterForRuntimeStartupFn> =
        match unsafe { lib.get(b"RegisterForRuntimeStartup\0") } {
            Ok(s) => s,
            Err(e) => return emit_error(format!("símbolo RegisterForRuntimeStartup não encontrado: {e}")),
        };

    let cmdline = format!("/usr/share/dotnet/dotnet {}", dll_file.to_str().unwrap_or_default());
    let mut cmdline_w = to_utf16(&cmdline);
    let cwd_w = to_utf16(cwd.to_str().unwrap_or("."));

    let mut pid: DWord = 0;
    let mut resume_handle: Handle = ptr::null_mut();
    let hr = unsafe {
        create_process_for_launch(
            cmdline_w.as_mut_ptr(),
            1, // bSuspendProcess = TRUE
            ptr::null_mut(),
            cwd_w.as_ptr(),
            &mut pid,
            &mut resume_handle,
        )
    };
    if hr != com::S_OK {
        return emit_error(format!("CreateProcessForLaunch falhou: hr=0x{:08x}", hr as u32));
    }

    let mut startup_token: *mut c_void = ptr::null_mut();
    let hr = unsafe {
        register_for_runtime_startup(pid, startup_callback, ptr::null_mut(), &mut startup_token)
    };
    if hr != com::S_OK {
        return emit_error(format!("RegisterForRuntimeStartup falhou: hr=0x{:08x}", hr as u32));
    }

    let hr = unsafe { resume_process(resume_handle) };
    if hr != com::S_OK {
        return emit_error(format!("ResumeProcess falhou: hr=0x{:08x}", hr as u32));
    }

    let attach_deadline = Instant::now() + Duration::from_secs(10);
    while !unsafe { CALLBACK_FIRED } {
        if Instant::now() > attach_deadline {
            return emit_error("timeout esperando o callback de RegisterForRuntimeStartup (10s)");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let startup_hr = unsafe { CALLBACK_HR };
    if startup_hr != com::S_OK {
        return emit_error(format!(
            "callback de RegisterForRuntimeStartup retornou hr=0x{:08x}",
            startup_hr as u32
        ));
    }

    let p_cordb = unsafe { P_CORDB };
    let icordebug_ptr = match unsafe { com::query_interface(p_cordb, &com::IID_ICORDEBUG) } {
        Ok(p) => p,
        Err(hr) => return emit_error(format!("QueryInterface(IID_ICORDEBUG) falhou: hr=0x{:08x}", hr as u32)),
    };

    let cordebug = com::CorDebug(icordebug_ptr);
    let hr = unsafe { cordebug.initialize() };
    if hr != com::S_OK {
        return emit_error(format!("ICorDebug::Initialize falhou: hr=0x{:08x}", hr as u32));
    }

    // ManagedCallbackObj precisa viver até o fim da sessão de debug — Box
    // "vazado" de propósito (processo inteiro morre no fim do worker de
    // qualquer forma, dentro do nsjail).
    let callback_obj = Box::new(com::ManagedCallbackObj {
        vtbl: &com::MANAGED_CALLBACK_VTBL,
        ref_count: 0,
    });
    let callback_ptr = Box::into_raw(callback_obj) as *mut c_void;

    let hr = unsafe { cordebug.set_managed_handler(callback_ptr) };
    if hr != com::S_OK {
        return emit_error(format!("ICorDebug::SetManagedHandler falhou: hr=0x{:08x}", hr as u32));
    }

    if let Err(hr) = unsafe { cordebug.debug_active_process(pid) } {
        return emit_error(format!("ICorDebug::DebugActiveProcess falhou: hr=0x{:08x}", hr as u32));
    }

    // A partir daqui, toda a inspeção/emite-evento acontece dentro dos
    // callbacks (com.rs) — só esperamos o processo terminar (ExitProcess).
    //
    // This used to also race its own userspace deadline against nsjail's
    // `--time_limit` and emit Event::Timeout itself if it won — but that's
    // exactly backwards: nsjail's own --time_limit SIGKILLs THIS process
    // (run_worker is the direct jailed child), so whichever fires first
    // wins unpredictably, and if nsjail wins we're dead before finishing
    // the emit anyway. Timeout detection now lives one level up, in
    // run_outer() (via events::run_nsjail), which is NOT jailed and so
    // reliably survives to observe nsjail's own kill and emit a clean event
    // — see run_outer's doc comment. This inner deadline stays only as a
    // safety net against a hang that isn't actually a --time_limit kill
    // (e.g. stuck waiting on ExitProcess for some other reason); it just
    // returns non-zero without emitting, since run_outer can't tell that
    // apart from a normal crash either way.
    //
    // Renamed from SPIKE_TIME_LIMIT to CSHARP_TIME_LIMIT_SECS, same as
    // RunOptions::default above — but note this read is UNAFFECTED by that
    // rename in practice: this line runs INSIDE the jail, and nsjail does
    // NOT forward the parent's environment by default (confirmed
    // empirically, see tasks.md "Cap de 5.000 eventos" / "Throttling"
    // finding on SPIKE_TIME_LIMIT's pre-existing non-forwarding — this var
    // was never added to run_outer's `--env` allowlist, unlike SPIKE_SAMPLE
    // which was fixed). So this always falls back to the hardcoded default
    // below regardless of what's set on the host — a known, documented gap,
    // not fixed by this task (would require adding `--env` forwarding here
    // too, changing scope from "pick a value" to "fix env propagation").
    // The fallback default is kept in sync with RunOptions::default's 15s
    // (see that comment for the real measurement behind the number) so that,
    // even though this specific read can't be tuned via the env var today,
    // its hardcoded value still matches the deliberately-chosen production
    // default rather than silently drifting from it.
    let run_timeout_secs: u64 = std::env::var("CSHARP_TIME_LIMIT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(15);
    let run_deadline = Instant::now() + Duration::from_secs(run_timeout_secs);
    loop {
        if unsafe { com::PROCESS_EXITED } {
            break;
        }
        // Checked here (not just after the loop, as FATAL_ERROR used to be)
        // so a callback-detected fatal condition — e.g. the multi-thread
        // block below — ends the run immediately instead of idling up to
        // the full deadline waiting for a PROCESS_EXITED that may never
        // come (the debuggee is still running when we detect this).
        if unsafe { com::FATAL_ERROR } {
            return 1;
        }
        if Instant::now() > run_deadline {
            return 1;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // Perf line, same shape/purpose as jdi/Debugger.java's final "[perf] ..."
    // line — lets sampling be validated the same way on both languages
    // (eventosTotais = every StepComplete callback, emitidos = only the
    // ones that actually paid extraction+emission cost per SAMPLE_N).
    unsafe {
        // Copy out of the `static mut`s into locals before formatting (not
        // `eprintln!("...", com::SAMPLE_N, ...)` directly) to avoid taking a
        // live shared reference into a mutable static, which 2024-edition
        // rustc warns on (`static_mut_refs`) — same non-issue in practice
        // (single-threaded at this point, nothing else writes these once
        // the debug session has ended) but no reason to introduce a new
        // warning the rest of this file doesn't already have.
        let elapsed_ms = RUN_START.map(|t0| t0.elapsed().as_millis() as u64).unwrap_or(0).max(1);
        let sample_n = com::SAMPLE_N;
        let total = com::STEP_EVENTS_TOTAL;
        let emitted = com::STEP_EVENTS_EMITTED;
        eprintln!(
            "[perf] sampleN={} eventosTotais={} emitidos={} tempo={}ms taxaTotal={:.1} ev/s taxaEmitida={:.1} ev/s",
            sample_n,
            total,
            emitted,
            elapsed_ms,
            total as f64 * 1000.0 / elapsed_ms as f64,
            emitted as f64 * 1000.0 / elapsed_ms as f64,
        );
    }

    // KNOWN GAP (documented in tasks.md, not fixed): `pid` here is the
    // DEBUGGEE (launched via dbgshim's CreateProcessForLaunch), a separate
    // OS process from run_worker itself. A cgroup OOM kill lands on
    // whichever process actually holds the memory — plausibly the
    // debuggee, not run_worker — in which case nsjail's own exit-code/
    // signal-based detection one level up (events::run_nsjail in
    // run_outer(), which only watches nsjail's DIRECT child, i.e. THIS
    // process) would see nothing wrong.
    //
    // The equivalent Java fix (Debugger.java checking
    // `vm.process().exitValue()` after VMDeathEvent) doesn't translate
    // here: tried reaping the debuggee ourselves via a raw `waitpid(pid,
    // ..., WNOHANG)` (pid is a real OS child of this process) both right
    // after ExitProcess and after falling through run_deadline. Tested
    // empirically by SIGKILLing the debuggee directly (same delivery
    // mechanism a cgroup OOM kill uses, from a separate process, mid-run):
    // ICorDebug's ExitProcess callback never fired at all (waited the full
    // 15s deadline, confirmed via logging — not just a reap race), and the
    // follow-up waitpid() also found nothing to reap. Whatever dbgshim uses
    // internally to track the debuggee doesn't surface this to our minimal
    // ICorDebugManagedCallback implementation, and there's no other
    // ICorDebug vtable slot implemented so far (or found) that exposes the
    // debuggee's raw exit status. So: memory_limit_exceeded for C# only
    // fires today when the OOM kill happens to land on run_worker itself
    // (covered by run_outer's events::run_nsjail, same as Timeout) — an
    // OOM kill that specifically targets the debuggee process is currently
    // indistinguishable from any other unexplained hang and just falls
    // through as a generic non-zero exit.
    if unsafe { com::FATAL_ERROR } {
        return 1;
    }
    0
}
