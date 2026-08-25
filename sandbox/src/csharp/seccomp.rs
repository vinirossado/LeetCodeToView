// Fase 2 hardening: minimal default-deny seccomp-bpf allowlist for the
// jailed process -- see java.rs's JAVA_SECCOMP_POLICY doc comment for the
// kafel syntax verification (same nsjail/kafel source, same `--help`
// check), shared here rather than repeated. Key difference from Java:
// there's no separate "javac equivalent" running outside the jail here --
// `dotnet build` happens on the API side (ProcessSandboxRunner, outside
// sandbox-runner entirely) before sandbox-runner is even invoked, so
// everything THIS binary does once re-exec'd into the jail (--csharp-worker,
// see csharp/worker.rs::run_worker) -- dlopen'ing libdbgshim.so, the
// dbgshim/ICorDebug handshake, AND launching+debugging the actual `dotnet
// <dll>` debuggee child process -- has to be covered by this one policy.
//
// Derived the same way as Java's: UNION of `strace -f` runs of the exact
// `/app/sandbox-runner --csharp-worker --dll <dll>` command line (same
// self-re-exec csharp/outer.rs performs, run directly instead of via
// nsjail) across every project in `test-snippets-csharp/`, PLUS two
// throwaway probes (not committed test-snippets -- the official C# suite
// has no equivalent of Java's NetworkEscape.java/FilesystemEscape.java)
// mirroring those two Java snippets, to check the same escape-attempt path
// empirically instead of leaving it silently unvalidated. Real finding from
// that probe, not a guess: a raw `Socket.Connect` in C# already gets
// blocked by THIS sandbox's own pre-existing multi-thread guard (>2
// distinct ICorDebug threads = blocked, MVP scope, see com/callback/mod.rs)
// before ever reaching a real connect() syscall -- .NET's socket layer
// lazily spins up an internal epoll-readiness thread on first use, which
// trips that guard first. So unlike Java, raw sockets in C# are already a
// dead end today for a reason entirely unrelated to seccomp, and this
// policy does NOT need connect/accept/etc. -- omitting them doesn't
// introduce any new crash class, the attempt already ends in a caught
// "error" event either way. (The filesystem half of that probe --
// File.ReadAllText("/etc/shadow"), File.WriteAllText outside cwd -- needed
// nothing beyond what the official suite already exercises.) Same
// arm64-derivation caveat as java.rs's JAVA_SECCOMP_POLICY applies here too
// -- not verified on amd64.
pub(super) const CSHARP_SECCOMP_POLICY: &str = r#"ALLOW {
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
    // reason, confirmed via its own isolated strace.
    symlinkat, renameat, fchmodat, utimensat,

    // Real gap found and fixed (tasks.md, uncaught-exception hang
    // investigation): once the ICorDebugManagedCallback2/QueryInterface fix
    // resolved the debugger-side deadlock that was masking this, an END-TO-
    // END real run (POST /executions, real nsjail, not a throwaway probe)
    // STILL hung -- confirmed via `strace -f` attached to the real nsjail
    // invocation (not guessed): the target process printed the real
    // exception text (CoreCLR builds it via `System.Diagnostics.
    // StackTrace`/`System.Reflection.Metadata`, which needs `flock(pdb_fd,
    // LOCK_SH|LOCK_NB)` on the program's own `.pdb` while resolving the
    // stack trace's source locations) and then, after printing, terminates
    // an unhandled managed exception the same way native Unix crash
    // reporting does: `tgkill(pid, tid, SIGABRT)` on itself. NEITHER
    // syscall was in this policy -- both `strace` outputs showed `= ?`
    // (indeterminate, syscall intercepted by seccomp, process never
    // resumes) at exactly those two calls, in that order, confirmed by
    // adding each ONE AT A TIME (flock alone got the exception text to
    // print but the process still hung at tgkill; adding tgkill too let
    // ExitProcess fire and the run complete cleanly, `docker service
    // logs`-style real end-to-end validation, not inferred from source
    // reading).
    flock, tgkill,

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
    // detection (StackOverflowCs -- see csharp/outer.rs's run_outer doc
    // comment on the "Stack overflow." stderr marker), on top of the same
    // rt_sigaction/rt_sigprocmask/rt_sigreturn set Java needs.
    sigaltstack, rt_sigaction, rt_sigprocmask, rt_sigreturn, restart_syscall,

    // epoll: .NET's socket/thread-pool readiness engine initializes lazily
    // on first async use (observed via the throwaway network probe, see
    // this const's doc comment) -- kept since other legitimate async
    // paths (Task, Timer, thread-pool work items) can trigger the same
    // lazy init even without raw sockets.
    epoll_create1, epoll_pwait
} DEFAULT KILL"#;
