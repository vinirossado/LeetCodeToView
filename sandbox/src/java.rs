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
        // and the target JVM launched below — harmless (heap/metaspace
        // limits are already pinned explicitly via -Xmx/-XX:MaxMetaspaceSize,
        // not cgroup-autodetected) but it shares this process's real stdout
        // with the sandboxed program's own output (see events::run_nsjail),
        // so it was leaking into the user-facing stdout panel. This flag
        // only silences that log tag, it doesn't disable container support.
        "-Xlog:os+container=off",
        "-XX:CompressedClassSpaceSize=64m",
        "-Xmx128m",
        &format!("-Dspike.sample={}", opts.sample_n),
        "-cp", "/app/jdi-out",
        "Debugger",
        class_name,
        &format!(
            "-Xlog:os+container=off -XX:CompressedClassSpaceSize=64m -cp {} -Xmx256m -XX:MaxMetaspaceSize=64m",
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
