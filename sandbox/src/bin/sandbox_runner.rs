// Binário unificado do Sandbox Runner — dispatcha pra java.rs, csharp.rs ou
// ruby.rs conforme --language, emitindo o mesmo schema de evento JSON
// (events.rs) pras três linguagens.

use sandbox_runner_lib::{csharp, java, ruby};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Modo "worker" do C#: o próprio nsjail já executou este binário de
    // novo, já isolado — pula o dispatcher normal e vai direto pra lógica
    // de debug (ver csharp::run_outer / csharp::run_worker).
    if args.iter().any(|a| a == csharp::CSHARP_WORKER_FLAG) {
        let dll = arg_value(&args, "--dll").expect("--csharp-worker precisa de --dll <path>");
        std::process::exit(csharp::run_worker(&PathBuf::from(dll)));
    }

    let language = arg_value(&args, "--language").unwrap_or_default();
    let file = arg_value(&args, "--file");

    match language.as_str() {
        "java" => {
            let file = file.expect("--language java precisa de --file <arquivo.java>");
            let status = java::run(&PathBuf::from(file), &java::RunOptions::default());
            std::process::exit(status.code().unwrap_or(1));
        }
        "csharp" => {
            let file = file.expect("--language csharp precisa de --file <arquivo.dll>");
            let status = csharp::run_outer(&PathBuf::from(file), &csharp::RunOptions::default());
            std::process::exit(status.code().unwrap_or(1));
        }
        "ruby" => {
            let file = file.expect("--language ruby precisa de --file <arquivo.rb>");
            let status = ruby::run(&PathBuf::from(file), &ruby::RunOptions::default());
            std::process::exit(status.code().unwrap_or(1));
        }
        _ => {
            eprintln!("uso: sandbox-runner --language java|csharp|ruby --file <arquivo>");
            std::process::exit(1);
        }
    }
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
