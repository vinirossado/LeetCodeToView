// Schema de evento compartilhado entre Java (JDI) e C# (ICorDebug) — ver
// spec.md "Eventos de execução". As duas linguagens devem emitir exatamente
// o mesmo formato, pra API/frontend não precisarem saber qual runtime gerou.

use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;

#[derive(Serialize)]
#[serde(tag = "type")]
pub enum Event {
    #[serde(rename = "step")]
    Step {
        line: i64,
        locals: BTreeMap<String, serde_json::Value>,
        stack: Vec<String>,
        time_ns: Option<u64>,
        memory_bytes: Option<u64>,
    },
    #[serde(rename = "timeout")]
    Timeout,
    #[serde(rename = "memory_limit_exceeded")]
    MemoryLimitExceeded,
    #[serde(rename = "output_truncated")]
    OutputTruncated,
    #[serde(rename = "stack_overflow")]
    StackOverflow,
    #[serde(rename = "step_limit_exceeded")]
    StepLimitExceeded,
    #[serde(rename = "error")]
    Error { message: String },
}

/// Emite 1 linha de evento JSON em stdout (formato JSONL — 1 objeto por
/// linha), que é o que a API/Sandbox Controller vão consumir.
pub fn emit(event: &Event) {
    if let Ok(json) = serde_json::to_string(event) {
        let stdout = std::io::stdout();
        let mut lock = stdout.lock();
        let _ = writeln!(lock, "{json}");
        let _ = lock.flush();
    }
}

/// Cap de eventos de step decidido na Fase 0.5 (ver spec.md) — igual pras
/// duas linguagens.
pub const STEP_EVENT_CAP: u32 = 5000;

// --- Fase 2 hardening: wrapping the nsjail child to turn opaque kills into
// clean events (Timeout, MemoryLimitExceeded, OutputTruncated) — shared by
// java.rs and csharp.rs, since both spawn nsjail the same way and need the
// same post-mortem classification. ---

/// Byte cap on total stdout forwarded from the jailed process before we
/// kill it and emit `output_truncated`. This stream carries BOTH the
/// program's own stdout (println/Console.WriteLine) AND our own step JSON
/// events — the instrumented driver (jdi/Debugger.java, or com.rs's
/// STEP_SINK for C#) writes events to the same stdout it inherits from the
/// target program, so there is no clean way to cap only "user output"
/// without parsing every line as JSON first. A legitimate run that hits the
/// 5,000-step cap (STEP_EVENT_CAP) with non-trivial locals/stack can
/// plausibly produce a few MB of step JSON alone, so this cap is set an
/// order of magnitude above that — it exists to catch genuinely unbounded
/// output (a `println`/`Console.WriteLine` in an infinite loop, which would
/// otherwise grow without bound), not to shave off legitimate step traces.
pub const OUTPUT_BYTE_CAP: usize = 10 * 1024 * 1024; // 10 MB

/// Marker nsjail itself logs (to stderr) right before delivering SIGKILL
/// for a `--time_limit` timeout. Confirmed empirically (built the sandbox
/// image, ran an infinite loop under nsjail directly): nsjail prints
/// `pid=N run time >= time limit (T >= T) (...). Killing it` immediately
/// followed by `terminated with signal: SIGKILL (9)`, and exits 137.
///
/// There is no equivalent marker for a cgroup OOM kill: reading nsjail's
/// own source (cgroup2.cc/subproc.cc) shows the kernel delivers SIGKILL to
/// the cgroup directly — nsjail never observes or logs anything OOM
/// specific, it just sees the same "terminated with signal: SIGKILL" as any
/// other externally-delivered SIGKILL. So a bare SIGKILL *without* this
/// marker is treated as "likely OOM" by process of elimination: in this
/// sandbox's nsjail invocation, cgroup_mem_max is the only other configured
/// mechanism able to deliver an unannounced SIGKILL to the jail. This is a
/// best-effort inference, not a certainty — documented as such in
/// tasks.md.
const NSJAIL_TIMEOUT_MARKER: &str = "run time >= time limit";

/// What `run_nsjail` determined happened to the child, beyond the raw exit
/// status. `Normal` doesn't rule out a language-specific crash (e.g. Java's
/// "StackOverflowError") — callers should still scan `RunResult::stderr_lines`
/// for those; this enum only covers what's detectable at the nsjail/OS
/// level, shared across languages.
pub enum RunOutcome {
    Normal,
    TimedOut,
    /// SIGKILL with no time_limit marker — see NSJAIL_TIMEOUT_MARKER doc.
    LikelyOom,
    OutputTruncated,
}

pub struct RunResult {
    pub status: ExitStatus,
    pub outcome: RunOutcome,
    /// Every stderr line the child produced, already relayed live to our
    /// own stderr (same visibility plain `Stdio::inherit()` gave before —
    /// ProcessSandboxRunner on the API side still sees everything). Kept
    /// around so callers can scan for language-specific crash markers.
    pub stderr_lines: Vec<String>,
}

/// Spawns an nsjail `Command` (stdout/stderr must NOT already be configured
/// on it — this function owns both) and blocks until the child is done.
/// Relays both streams live to our own stdout/stderr, exactly like
/// `Stdio::inherit()` did before, while watching stdout for the output cap
/// and stderr for the timeout marker. If the cap is hit, kills the child
/// immediately (it may be in an infinite print loop and never exit on its
/// own) instead of waiting for nsjail's own `--time_limit` to eventually
/// catch it.
pub fn run_nsjail(mut cmd: Command) -> RunResult {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("falha ao rodar nsjail (está instalado e no PATH?)");

    let stdout = child.stdout.take().expect("stdout não foi piped");
    let stderr = child.stderr.take().expect("stderr não foi piped");

    let (truncated_tx, truncated_rx) = mpsc::channel();
    let stdout_thread = thread::spawn(move || {
        let mut reader = stdout;
        let mut total: usize = 0;
        let mut truncated = false;
        let mut buf = [0u8; 8192];
        // Tracks whether the byte most recently written to our own stdout
        // was a newline -- see the truncated branch below for why.
        let mut last_byte_was_newline = true;
        let stdout_handle = std::io::stdout();
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            {
                let mut lock = stdout_handle.lock();
                let _ = lock.write_all(&buf[..n]);
                let _ = lock.flush();
            }
            last_byte_was_newline = buf[n - 1] == b'\n';
            total += n;
            if total > OUTPUT_BYTE_CAP {
                truncated = true;
                break;
            }
        }
        if truncated && !last_byte_was_newline {
            // Real bug found empirically while validating Ruby's
            // OutputFlood.rb through the actual API (not caught by the
            // same scenario run directly against sandbox-runner in
            // isolation, which happened to hit a chunk boundary that
            // landed on a newline by chance -- see tasks.md): this relay
            // forwards RAW 8192-byte chunks from the jailed child's
            // stdout, with no regard for line boundaries. When the
            // OUTPUT_BYTE_CAP is crossed mid-chunk, the cap can land
            // mid-LINE too (a jailed program's output lines rarely divide
            // evenly into 8192 bytes) -- the relayed stream then does NOT
            // end in a newline. `java.rs`/`csharp.rs`/`ruby.rs` each call
            // `events::emit(&Event::OutputTruncated)` right after this
            // function returns, which writes `{"type":"output_truncated"}\n`
            // as its own `writeln!` -- but with no separating newline
            // already in the stream, that JSON line ends up CONCATENATED
            // onto the tail of the truncated line instead of starting a
            // fresh one (confirmed via a real `POST /executions` against
            // the real API: the stored trace's second-to-last event was a
            // single `stdout` event whose text ended in
            // `...xxx{"type":"output_truncated"}` -- ExecutionJob's
            // `parseEventOrStdout` correctly falls through to treating that
            // whole garbled line as plain stdout text, since it isn't valid
            // JSON on its own, which is exactly the "misclassified as
            // plain stdout" failure mode this project has already hit once
            // for Java's TAB-escaping bug). This is NOT Ruby-specific --
            // the exact same relay code path is shared by java.rs/csharp.rs
            // too, just apparently never landed on this exact byte-count
            // coincidence during those languages' own validation. Fixed
            // here, once, centrally, rather than in each of the three
            // `run()` functions separately: write one extra newline before
            // handing control back, so whichever language-specific
            // `events::emit(...)` call comes next always starts on a clean
            // line, regardless of where the 8192-byte chunk boundary
            // happened to fall.
            let mut lock = stdout_handle.lock();
            let _ = lock.write_all(b"\n");
            let _ = lock.flush();
        }
        let _ = truncated_tx.send(truncated);
        if truncated {
            // Keep draining (without relaying) so the still-running child
            // doesn't block on a full pipe in the moment between us
            // deciding to truncate and the main thread actually killing it.
            while !matches!(reader.read(&mut buf), Ok(0) | Err(_)) {}
        }
    });

    let (stderr_tx, stderr_rx) = mpsc::channel();
    let stderr_thread = thread::spawn(move || {
        let mut lines = Vec::new();
        let mut saw_timeout_marker = false;
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            eprintln!("{line}");
            if line.contains(NSJAIL_TIMEOUT_MARKER) {
                saw_timeout_marker = true;
            }
            lines.push(line);
        }
        let _ = stderr_tx.send((lines, saw_timeout_marker));
    });

    // Blocks until stdout closes — either the child exited naturally (EOF),
    // or the reader thread hit the cap and stopped forwarding. Either way
    // this is the right moment to reap (and, if truncated, kill first).
    let truncated = truncated_rx.recv().unwrap_or(false);
    if truncated {
        let _ = child.kill();
    }
    let status = child.wait().expect("falha ao esperar nsjail");
    let _ = stdout_thread.join();
    let (stderr_lines, saw_timeout_marker) = stderr_thread
        .join()
        .ok()
        .and_then(|_| stderr_rx.recv().ok())
        .unwrap_or_default();

    let outcome = if truncated {
        RunOutcome::OutputTruncated
    } else if saw_timeout_marker {
        RunOutcome::TimedOut
    } else if status.signal() == Some(9) {
        RunOutcome::LikelyOom
    } else {
        RunOutcome::Normal
    };

    RunResult { status, outcome, stderr_lines }
}

/// Recursively chmods everything under `root` so the nsjailed process can
/// read (and, for directories, traverse) it after nsjail maps it to a
/// non-root uid/gid (see java.rs/csharp.rs's `--uid_mapping`/`--gid_mapping`).
///
/// Needed because the API (`ProcessSandboxRunner`, running as root) creates
/// the per-execution workdir via Java's `Files.createTempDirectory`, which
/// deliberately ignores the process umask and creates directories `0700`
/// (owner-only) for security — reasonable for a plain temp dir, but it means
/// a process mapped to a different, unprivileged uid gets `EACCES` on
/// `chdir()` into it. Confirmed empirically, not assumed: nsjail failed with
/// `chdir(...): Permission denied` the first time `--uid_mapping` was added,
/// before this existed. `0755` for directories (traverse+read) and `0644`
/// for files (read) — sandboxed code never needs to write back into its own
/// source/workdir, only read it and produce stdout.
pub fn make_world_readable(root: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(root)?;
    let mode = if metadata.is_dir() { 0o755 } else { 0o644 };
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(mode))?;

    if metadata.is_dir() {
        for entry in std::fs::read_dir(root)? {
            make_world_readable(&entry?.path())?;
        }
    }
    Ok(())
}
