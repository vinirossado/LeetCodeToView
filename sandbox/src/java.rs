// Runtime Java (JDI) — evolução direta do spike da Fase 0.5. O processo
// nsjail lançado aqui roda jdi/Debugger.java, que já emite JSON de evento
// diretamente no stdout (herdado, não passa pelo módulo `events` — ver
// TODO abaixo sobre alinhar o schema hand-rolled do Debugger.java com
// events::Event::Step, incluindo time_ns/memory_bytes que ainda faltam lá).

use std::env;
use std::path::Path;
use std::process::{Command, Stdio};

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

    Command::new("nsjail")
        .args([
            "--mode", "o",
            "--time_limit", &opts.time_limit_secs,
            "--rlimit_as", "3072",
            "--rlimit_cpu", "10",
            "--rlimit_nproc", "512",
            "--rlimit_nofile", "1024",
            "--use_cgroupv2",
            "--cgroup_mem_max", "536870912",
            "--cgroup_pids_max", "512",
            "--chroot", "/",
            "--cwd", src_dir.to_str().unwrap(),
            "--quiet",
            "--",
            "/usr/bin/java",
            "-XX:CompressedClassSpaceSize=64m",
            "-Xmx128m",
            &format!("-Dspike.sample={}", opts.sample_n),
            "-cp", "/app/jdi-out",
            "Debugger",
            class_name,
            &format!(
                "-XX:CompressedClassSpaceSize=64m -cp {} -Xmx256m -XX:MaxMetaspaceSize=64m",
                src_dir.to_str().unwrap()
            ),
        ])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .expect("falha ao rodar nsjail (está instalado e no PATH?)")
}
