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
use std::time::Duration;

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
