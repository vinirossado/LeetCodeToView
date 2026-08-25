// Chamado pelo dispatcher normal (fora do jail): compila o projeto (feito
// pela API, antes de chegar aqui) e faz fork+exec do nsjail apontando pro
// nosso próprio binário com `--csharp-worker`, que vai rodar
// csharp::worker::run_worker() já isolado. Ver csharp/mod.rs pro overview
// do padrão "self re-exec".

use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, ExitStatus};

use crate::events::{self, Event, RunOutcome};

use super::seccomp::CSHARP_SECCOMP_POLICY;
use super::CSHARP_WORKER_FLAG;

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
        // see CSHARP_SECCOMP_POLICY's doc comment (csharp/seccomp.rs) for
        // how this syscall set was derived and validated.
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
    let mut status = result.status;
    match result.outcome {
        RunOutcome::TimedOut => events::emit(&Event::Timeout),
        RunOutcome::LikelyOom => events::emit(&Event::MemoryLimitExceeded),
        RunOutcome::OutputTruncated => events::emit(&Event::OutputTruncated),
        RunOutcome::Normal => {
            if result.stderr_lines.iter().any(|l| l.contains("Stack overflow.")) {
                events::emit(&Event::StackOverflow);
            } else if let Some(start) = result.stderr_lines.iter().position(|l| l.starts_with("Unhandled exception.")) {
                // Real bug found and fixed (tasks.md, same investigation
                // that found the flock/tgkill seccomp gap above): once the
                // debugger-side deadlock AND the seccomp gap were both
                // fixed, an uncaught C# exception could for the first time
                // ever actually reach here — exposing a THIRD, previously
                // dormant bug, the exact same class already fixed for Java
                // earlier this session ("any uncaught exception silently
                // reported as completed"): cb_exit_process (com/callback/
                // mod.rs) only sets PROCESS_EXITED, never inspects HOW the
                // target died, and run_worker's final `0` return doesn't
                // check for that either — so a target that self-terminates
                // via `tgkill(..., SIGABRT)` after printing its exception
                // (CoreCLR's real, confirmed-via-strace unhandled-exception
                // termination path) looked identical to a clean exit.
                // Confirmed via a real `POST /executions` end-to-end before
                // this fix: `status: "completed"`, trace silently truncated
                // right before the crash, no error event, no "after" -- the
                // exact same silent-success bug, just with a different,
                // previously- unreachable root cause than the Java one had.
                //
                // Fix: CoreCLR's own unhandled-exception message is already
                // real, useful, user-facing text (it's the user's OWN
                // program's exception, not sandbox internals -- same
                // "safe and useful to show verbatim" reasoning as Java's
                // identical fix) -- printed to stderr starting with the
                // literal `Unhandled exception.` line, confirmed via
                // `strace`.
                //
                // Real corruption found and worked around, not assumed
                // clean: this driver's own `eprintln!("[callback] ...")`
                // tracing (com/callback/mod.rs) shares the SAME inherited
                // stderr fd as the debuggee -- both processes write to it
                // directly, with no line-atomicity guarantee between them
                // (same class of interleaving bug already found and fixed
                // generically for Ruby's stdout relay earlier this session,
                // just a different pair of writers here). Confirmed via a
                // real `POST /executions` end-to-end, reproduced via
                // sandbox-runner's own real entrypoint (not a synthetic
                // probe): CoreCLR's own `write()` of "Unhandled exception. "
                // (no trailing newline in that specific write) landed on
                // the SAME line as this driver's very next `[callback]`
                // trace line -- but critically, the exception's OWN
                // type/message/stack trace (separate, later `write()` calls
                // from CoreCLR, each newline-terminated) came through
                // CLEAN on their own lines further down the stream, just
                // separated from that first corrupted line by many
                // unrelated pure `[callback] LoadAssembly!`/`LoadModule!`
                // lines in between (module loads CoreCLR does while
                // building the exception report). An earlier version of
                // this fix used `take_while` and stopped at the very next
                // pure `[callback]` line, before ever reaching the real
                // message/stack trace further down -- confirmed via a real
                // run producing just `"Unhandled exception. "` with
                // everything real missing. Fixed by FILTERING (not
                // stopping at) lines that are entirely this driver's own
                // trace output, and truncating (not dropping) any line
                // where the marker appears mid-line -- keeps scanning the
                // whole captured window instead of bailing out at the
                // first noise line.
                let message: String = result.stderr_lines[start..]
                    .iter()
                    .filter(|l| !l.starts_with("[callback]") && !l.starts_with("[perf]"))
                    .map(|l| l.split("[callback]").next().unwrap_or(l.as_str()).trim_end())
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                events::emit(&Event::Error { message });
                // Emitting the event alone isn't enough: ExecutionJob.java
                // (API side) decides completed-vs-failed PURELY from this
                // process's own exit code (a non-zero exit throws inside
                // ProcessSandboxRunner, which is the only path that sets
                // ExecutionStatus.FAILED — see that file's own doc comment)
                // -- it never inspects event content on the success path.
                // run_worker's own exit code stays 0 here (PROCESS_EXITED
                // fired, no FATAL_ERROR condition applies -- the target
                // exiting via an uncaught exception isn't one of the
                // conditions run_worker itself currently checks for), so
                // without this override the real, correctly-emitted error
                // event above would still be silently reported as `status:
                // "completed"` -- confirmed via a real end-to-end run before
                // this exact line was added. `ExitStatus::from_raw(1 << 8)`
                // synthesizes a normal (non-signaled) exit with code 1, the
                // same convention `main.rs`'s `status.code().unwrap_or(1)`
                // already expects from every other failure path in this
                // codebase.
                status = ExitStatus::from_raw(1 << 8);
            }
        }
    }
    status
}
