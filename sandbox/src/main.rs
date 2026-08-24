use std::env;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

// Spike da Fase 0.5: valida se nsjail consegue isolar e rodar um .java
// com limites básicos, com o driver JDI (jdi/Debugger.java) instrumentando
// a execução e emitindo eventos de step em JSON. Debugger e alvo rodam no
// mesmo processo/jail (LaunchingConnector) — separar debugger (confiável)
// do alvo (isolado) é uma questão em aberto, não resolvida aqui.
// Flags de hardening completo (seccomp, chroot pra rootfs mínimo, usuário
// não-root) ficam pra Fase 2, conforme tasks.md — aqui o objetivo é só
// responder se o desenho faz sentido.

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("uso: sandbox-spike <arquivo.java>");
        std::process::exit(1);
    }
    let java_file = &args[1];

    let class_name = Path::new(java_file)
        .file_stem()
        .and_then(|s| s.to_str())
        .expect("nome de arquivo inválido");

    let src_dir = Path::new(java_file)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    eprintln!("[spike] compilando {java_file}...");
    let compile = Command::new("javac")
        .args(["-encoding", "UTF-8", "-g", java_file]) // -g: mantém info de linha/variáveis locais, que o JDI precisa
        .status()
        .expect("falha ao rodar javac");
    if !compile.success() {
        eprintln!("[spike] falha na compilação");
        std::process::exit(1);
    }

    eprintln!("[spike] rodando {class_name} isolado via nsjail...");
    let start = Instant::now();

    let status = Command::new("nsjail")
        .args([
            "--mode", "o", // once: roda uma vez e sai
            "--time_limit", &env::var("SPIKE_TIME_LIMIT").unwrap_or_else(|_| "10".into()),
            "--rlimit_as", "3072", // MB de address space — JVM reserva virtual memory bem além do heap (compressed class space ~1GB default, code cache, metaspace); RLIMIT_AS não bate 1:1 com RSS real usado
            "--rlimit_cpu", "10", // segundos de CPU
            "--rlimit_nproc", "512", // JVM sozinha já usa várias threads internas (GC, JIT)
            "--rlimit_nofile", "1024", // causa real da falha de criação de thread: default do nsjail é 32 fds, e cada thread Java abre fds (pipe/eventfd) pra sincronização
            "--use_cgroupv2",
            "--cgroup_mem_max", "536870912", // 512MB — folga pra 2 JVMs (debugger + alvo via LaunchingConnector) rodando ao mesmo tempo
            "--cgroup_pids_max", "512",
            "--chroot", "/", // rootfs mínimo fica pra Fase 2; aqui reusa o do container
            "--cwd", src_dir.to_str().unwrap(),
            "--quiet",
            "--",
            "/usr/bin/java",
            "-XX:CompressedClassSpaceSize=64m", // default é 1GB; com 2 JVMs (debugger + alvo) isso sozinho já estoura o rlimit_as
            "-Xmx128m",
            &format!("-Dspike.suspend={}", env::var("SPIKE_SUSPEND").unwrap_or_default()),
            &format!("-Dspike.skipdata={}", env::var("SPIKE_SKIPDATA").unwrap_or_default()),
            &format!("-Dspike.sample={}", env::var("SPIKE_SAMPLE").unwrap_or_else(|_| "1".into())),
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
        .expect("falha ao rodar nsjail (está instalado e no PATH?)");

    let elapsed = start.elapsed();
    eprintln!(
        "[spike] execução finalizada em {:?}, status: {}",
        elapsed, status
    );
}
