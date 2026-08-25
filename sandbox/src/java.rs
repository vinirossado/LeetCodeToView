// Runtime Java (JDI) — evolução direta do spike da Fase 0.5. O processo
// nsjail lançado aqui roda jdi/Debugger.java, que já emite JSON de evento
// diretamente no stdout (herdado, não passa pelo módulo `events` — é um
// processo Java separado, não tem como chamar o enum Rust `events::Event`).
// O schema hand-rolled já bate campo a campo com `events::Event::Step`
// (line/locals/stack/time_ns/memory_bytes) e já aplica o mesmo cap de 5.000
// eventos (`events::STEP_EVENT_CAP`) emitindo `step_limit_exceeded` ao
// atingir — ver jdi/Debugger.java. `memory_bytes` fica sempre `null`: não
// há hoje um jeito de ler o heap do processo alvo (lançado à parte via
// `LaunchingConnector`) sem JMX-over-JDWP, não implementado.
//
// Fase 2 hardening: nsjail is now run via `events::run_nsjail` (not a bare
// `.status()` with everything inherited) so we can turn opaque kills/crashes
// into clean events on our own stdout before this process exits — see
// events.rs for the shared timeout/OOM/output-cap detection. Stack overflow
// is Java-specific: jdi/Debugger.java already redirects the *target* JVM's
// stderr into its own (see `redirectStream` in Debugger.java), which flows
// through nsjail's inherited stderr up to the stream `run_nsjail` captures
// here — so an uncaught StackOverflowError shows up as a normal JVM crash
// line ("Exception in thread \"main\" java.lang.StackOverflowError") in
// `stderr_lines`, regardless of whether user code catches/rethrows it.
// Confirmed empirically: see tasks.md "Fase 2".

use std::env;
use std::path::Path;
use std::process::Command;

use crate::events::{self, Event, RunOutcome};

// Fase 2 hardening: minimal default-deny seccomp-bpf allowlist for the
// jailed `java ... Debugger ...` process (JDI driver + target JVM launched
// via LaunchingConnector -- both run inside this SAME jail, see the module
// doc comment above). Kafel syntax (`ALLOW { syscall, ... } DEFAULT KILL`,
// `//` line comments) confirmed empirically, not from memory: cloned
// nsjail's own source (the same repo `sandbox/Dockerfile`/`Dockerfile.api`
// build nsjail from) and its `kafel/` submodule, and cross-checked against
// nsjail's own README ("Kafel policy syntax" section) and `kafel/samples/*.
// policy`. `--seccomp_string`/`--seccomp_policy` confirmed via `nsjail
// --help` inside the real image (not guessed) -- `-P`/`--seccomp_policy` is
// a file path, `--seccomp_string` is this same syntax inline; inline is
// used here (not a shipped file) to keep this the single source of truth
// next to the other hardcoded nsjail flags in this file, same convention.
//
// The syscall set below is the UNION of `strace -f` runs of this exact
// `java ... Debugger ...` command line (same args as the ones built below,
// minus the nsjail wrapper -- javac itself runs OUTSIDE the jail, before
// this policy is even in effect, see `run()` above) across EVERY snippet in
// `test-snippets/` (loops, recursion/stack-overflow, multi-thread, memory-
// heavy allocation, output flooding, filesystem/network escape attempts).
// Categorized here (not just a bare list) so a future change to what kind
// of Java program this sandbox supports has a map of WHY each group exists:
const JAVA_SECCOMP_POLICY: &str = r#"ALLOW {
    // Process/thread lifecycle. execve is required because the JDI driver
    // (jdi/Debugger.java) launches the TARGET jvm as a real child process
    // via LaunchingConnector -- confirmed in the strace as a second
    // process, not a thread. clone covers both that fork and every
    // JVM-internal thread (GC/JIT/compiler housekeeping threads, plus any
    // user Thread -- see MultiThread.java).
    execve, clone, exit, exit_group, wait4, kill,
    set_tid_address, set_robust_list, rseq, prctl,
    gettid, getpid, geteuid, getuid,
    // futex: NOT optional -- every JVM thread (GC/JIT/compiler, plus any
    // user Thread) synchronizes via futex, including a "single-threaded"
    // program (the JVM itself is never actually single-threaded). ppoll is
    // the JDWP socket wait-with-timeout. prlimit64 is the JVM reading its
    // own rlimits (RLIMIT_NOFILE/RLIMIT_AS etc., the ones this same nsjail
    // invocation sets) at startup to size internal pools.
    futex, ppoll, prlimit64,

    // Memory management: heap, thread stacks, JIT code cache, compressed
    // class space, direct ByteBuffers (MaxDirectMemorySize) -- exercised by
    // MemoryHog.java/BigCountLoop.java.
    brk, mmap, munmap, mprotect, madvise,

    // File I/O: reading class files (bootclasspath + /app/jdi-out + the
    // user's own .java source dir) and the driver's own stdout/stderr.
    // FilesystemEscape.java's read of /etc/shadow and write to /tmp use
    // these same syscalls (openat/read/write) -- nsjail's chroot/mount
    // setup, not seccomp, is what's supposed to gate what paths succeed,
    // same layering choice as the network case below.
    openat, read, write, close, pread64, lseek,
    // newfstat, not `fstat`: the JVM genuinely DOES issue a raw fstat(2) on
    // an already-open fd (syscall number 80 on aarch64's generic syscall
    // ABI, confirmed by directly stracing this exact nsjail+seccomp
    // invocation -- first cut of this policy used the bare `fstat` name,
    // which kafel's parser rejected ("Undefined identifier `fstat`"), then
    // ran WITHOUT that syscall and confirmed it really is needed: the
    // target JVM got `+++ killed by SIGSYS +++` on `fstat(4, ...)` reading
    // `.../lib/server/classes.jsa` (the CDS archive) with it missing.
    // kafel's generated aarch64 table (kafel/src/syscalls/aarch64_syscalls.c)
    // names this syscall `newfstat` (distinct from `newfstatat`, which
    // takes a directory-relative path and is what most other fd-metadata
    // calls compile down to on this architecture).
    newfstat, newfstatat, statx, statfs, faccessat, readlinkat,
    getdents64, getcwd, fchdir, mkdirat, unlinkat, ftruncate, flock,
    dup3, ioctl, fcntl, pipe2,

    // Sockets: the driver and target JVM talk JDWP over a loopback TCP
    // socket (JDI's CommandLineLaunch connector) -- required for the
    // debugger itself to work, not just user code. Also what
    // NetworkEscape.java's own connection attempt needs to even try:
    // letting that attempt fail at the network-NAMESPACE level (no
    // interfaces reachable, already validated to surface as a normal
    // caught IOException) is the correct isolation layer for user network
    // I/O -- NOT this seccomp policy, which would otherwise turn a
    // catchable exception into an uncatchable SIGSYS crash instead, a
    // strictly worse and inconsistent-with-Java outcome.
    socket, connect, bind, listen, accept,
    getsockname, getsockopt, setsockopt, shutdown, socketpair,
    sendto, recvfrom, recvmsg,

    // Time/scheduling/misc the JVM needs at startup and during normal
    // operation (System.nanoTime, GC pacing, thread scheduling hints,
    // /dev/urandom-backed SecureRandom seeding).
    clock_gettime, clock_getres, clock_nanosleep,
    sched_getaffinity, sched_yield, getrusage, sysinfo, getrandom,
    // newuname, not `uname` -- kafel's aarch64 syscall table names this
    // syscall `newuname` (matching the kernel's internal `sys_newuname`,
    // what libc's `uname()` actually maps to on this architecture); a bare
    // `uname` fails the same "Undefined identifier" compile check as the
    // `fstat` case above. Confirmed by reading the same generated table.
    newuname,

    // Signals: the JVM installs a SIGSEGV handler at startup as part of its
    // guard-page-based StackOverflowError detection (DeepRecursion.java/
    // StackOverflowNoCatch.java) and handles a few others (SIGTERM etc.)
    // for shutdown hooks.
    rt_sigaction, rt_sigprocmask, rt_sigreturn, restart_syscall
} DEFAULT KILL"#;

// Architecture caveat, same category as the arm64-only netcoredbg pin
// documented in csharp.rs: the syscall set above was derived on linux/arm64
// (Apple Silicon Docker Desktop, this project's dev environment) via a real
// `strace -f`, not guessed. aarch64 Linux only has the "modern" syscall
// forms (openat/faccessat/newfstatat/clone -- no legacy open/access/stat/
// fork), so an amd64 host's glibc *could* pick a different legacy syscall
// for the same libc call in some cases. Not verified on amd64 yet -- if
// this is ever deployed there, re-derive (or re-validate) this list on that
// architecture the same way, don't assume it transfers unchanged.

pub struct RunOptions {
    pub time_limit_secs: String,
    pub sample_n: String,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            time_limit_secs: env::var("SPIKE_TIME_LIMIT").unwrap_or_else(|_| "10".into()),
            sample_n: env::var("SPIKE_SAMPLE").unwrap_or_else(|_| "1".into()),
        }
    }
}

/// Compila e roda um .java isolado via nsjail, com o driver JDI instrumentando.
/// Eventos JSON saem no stdout do processo atual (herdado do child).
pub fn run(java_file: &Path, opts: &RunOptions) -> std::process::ExitStatus {
    let class_name = java_file
        .file_stem()
        .and_then(|s| s.to_str())
        .expect("nome de arquivo inválido");

    let src_dir = java_file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    eprintln!("[sandbox-runner/java] compilando {java_file:?}...");
    let compile = Command::new("javac")
        .args(["-encoding", "UTF-8", "-g", java_file.to_str().unwrap()])
        .status()
        .expect("falha ao rodar javac");
    if !compile.success() {
        eprintln!("[sandbox-runner/java] falha na compilação");
        std::process::exit(1);
    }

    // ProcessSandboxRunner (the API, running as root) creates the workdir
    // via Java's Files.createTempDirectory, which is 0700 regardless of
    // umask — unreadable by the non-root uid nsjail maps the jailed process
    // to below. See events::make_world_readable's doc comment.
    events::make_world_readable(src_dir).expect("falha ao ajustar permissões do workdir");

    eprintln!("[sandbox-runner/java] rodando {class_name} isolado via nsjail...");

    let mut cmd = Command::new("nsjail");
    cmd.args([
        "--mode", "o",
        "--time_limit", &opts.time_limit_secs,
        "--rlimit_as", "3072",
        // Was hardcoded to "10" independent of --time_limit — a latent
        // mismatch found while testing stack-overflow detection (Fase 2):
        // a CPU-bound target could get SIGKILLed by RLIMIT_CPU exhaustion
        // at a DIFFERENT time than nsjail's own --time_limit, and that
        // SIGKILL looks identical (exit 137, no nsjail marker) to a cgroup
        // OOM kill — see the ambiguity noted in Debugger.java and
        // csharp.rs's run_worker. Tying it to time_limit_secs (same pattern
        // already used in csharp.rs) removes one avoidable source of that
        // ambiguity, even though it doesn't eliminate it (RLIMIT_CPU is
        // still a distinct mechanism from --time_limit and from cgroup
        // memory, just no longer skewed to a different budget).
        "--rlimit_cpu", &opts.time_limit_secs,
        "--rlimit_nproc", "512",
        "--rlimit_nofile", "1024",
        "--use_cgroupv2",
        "--cgroup_mem_max", "536870912",
        // Fase 2 hardening: memory.swap.max=0 for the jail's cgroup, so a
        // memory-hungry target gets SIGKILLed by cgroup_mem_max immediately
        // instead of swapping (which would thrash the HOST, not just the
        // jail — swap is host-wide, not namespaced per cgroup). Confirmed
        // by reading nsjail's own source (cgroup2.cc::initNsFromParentMem):
        // --cgroup_mem_swap_max VALUE writes VALUE straight to the cgroup
        // v2 `memory.swap.max` file (default is -1, meaning "don't write
        // it at all" — NOT "swap allowed", so this flag is required, not
        // redundant with cgroup_mem_max). See tasks.md "Fase 2" for the
        // empirical verification (reading memory.swap.max from inside a
        // running jail) and the same Docker Desktop/macOS cgroup-delegation
        // caveat already documented for cgroup_mem_max.
        "--cgroup_mem_swap_max", "0",
        "--cgroup_pids_max", "512",
        // Non-root inside the jail (Fase 2 "Sem privilégios" — see
        // tasks.md). NOT nsjail's simpler --user/--group: those always map
        // the inside uid back to nsjail's OWN (root) uid outside the
        // namespace, and nsjail's own log even warns about it ("will have
        // user root-level access to files") — confirmed empirically with a
        // manual nsjail run, not just from the warning text: the process
        // still had genuine root-level DAC access despite getuid()/ps
        // showing "nobody". --uid_mapping/--gid_mapping instead map to a
        // REAL unprivileged uid (65534, nobody/nogroup) outside the
        // namespace too — validated with the same manual test, no warning,
        // and a real Java program (reading its own source file, spawning a
        // subprocess) ran correctly with genuine uid=65534/gid=65534
        // throughout. Requires newuidmap/newgidmap + /etc/subuid/subgid
        // delegation for root (see Dockerfile.api) — nsjail shells out to
        // those even though it's already running as real root.
        "--uid_mapping", "65534:65534:1",
        "--gid_mapping", "65534:65534:1",
        // Fase 2 hardening: minimal default-deny seccomp-bpf allowlist --
        // see JAVA_SECCOMP_POLICY's doc comment above for how this syscall
        // set was derived and validated.
        "--seccomp_string", JAVA_SECCOMP_POLICY,
        "--chroot", "/",
        "--cwd", src_dir.to_str().unwrap(),
        // NOT --quiet: nsjail's own INFO-level log line ("run time >= time
        // limit ... Killing it") is the ONLY way to tell a --time_limit
        // kill apart from a cgroup OOM kill — both end up as the exact same
        // exit code (128+SIGKILL=137, see nsjail's reapProc: `return 128 +
        // WTERMSIG(status)`), so --quiet (which drops LOG_I) would make the
        // two indistinguishable. See events::run_nsjail /
        // NSJAIL_TIMEOUT_MARKER. Confirmed empirically (see tasks.md
        // "Fase 2") — with --quiet, the marker line never reaches our
        // stderr-scanning code at all.
        "--",
        "/usr/bin/java",
        // -Xlog:os+container=off: nsjail's --chroot / cgroup-per-jail setup
        // (cgroup path 'NSJAIL.<pid>' instead of a normal container path)
        // makes the JVM's own container-detection log a *guaranteed*
        // "[warning][os,container] Cgroup ... controller path ... seems to
        // have moved ..." on every single run, for both the driver JVM here
        // and the target JVM launched below — harmless (heap/metaspace/
        // direct-memory limits are already pinned explicitly via -Xmx/
        // -XX:MaxMetaspaceSize/-XX:MaxDirectMemorySize, not cgroup-
        // autodetected) but it shares this process's real stdout
        // with the sandboxed program's own output (see events::run_nsjail),
        // so it was leaking into the user-facing stdout panel. This flag
        // only silences that log tag, it doesn't disable container support.
        "-Xlog:os+container=off",
        "-XX:CompressedClassSpaceSize=64m",
        "-Xmx128m",
        // -XX:MaxDirectMemorySize: off-heap memory (java.nio.ByteBuffer.
        // allocateDirect, usable by sandboxed user code) is NOT covered by
        // -Xmx at all — it's a separate allocator. Empirically confirmed
        // (not assumed) that HotSpot already defaults this to match -Xmx
        // when unset (tested inside this exact container: -Xmx256m alone
        // already capped ByteBuffer.allocateDirect at exactly 256MB,
        // -Xmx100m at exactly 100MB) — so this was never actually an open
        // bypass, just an implicit, undocumented-at-the-call-site behavior
        // this codebase would rather not depend on silently. Set explicitly
        // here anyway, matching each JVM's own -Xmx, so the limit is a
        // visible invariant instead of something a future JDK version (or
        // someone changing -Xmx without knowing about this coupling) could
        // silently break.
        "-XX:MaxDirectMemorySize=128m",
        &format!("-Dspike.sample={}", opts.sample_n),
        "-cp", "/app/jdi-out",
        "Debugger",
        class_name,
        &format!(
            "-Xlog:os+container=off -XX:CompressedClassSpaceSize=64m -cp {} -Xmx256m -XX:MaxMetaspaceSize=64m -XX:MaxDirectMemorySize=256m",
            src_dir.to_str().unwrap()
        ),
    ]);

    let result = events::run_nsjail(cmd);
    match result.outcome {
        RunOutcome::TimedOut => events::emit(&Event::Timeout),
        RunOutcome::LikelyOom => events::emit(&Event::MemoryLimitExceeded),
        RunOutcome::OutputTruncated => events::emit(&Event::OutputTruncated),
        RunOutcome::Normal => {
            if result.stderr_lines.iter().any(|l| l.contains("StackOverflowError")) {
                events::emit(&Event::StackOverflow);
            }
        }
    }
    result.status
}
