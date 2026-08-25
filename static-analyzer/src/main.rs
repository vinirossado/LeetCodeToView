mod csharp_adapter;
mod engine;
mod ir;
mod java_adapter;
mod ruby_adapter;

use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

/// Language is picked from the file extension (`.java` vs `.cs`) — the CLI is
/// already invoked with a source file path, so this needs no new flag and stays
/// consistent with how the API's `ProcessStaticAnalyzer` will eventually invoke it
/// (it already receives a `language` string per request, so mapping that to an
/// extension when writing the temp file is a one-line concern on that side, not
/// here).
enum Language {
    Java,
    CSharp,
    Ruby,
}

fn detect_language(path: &str) -> Result<Language, String> {
    match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some("java") => Ok(Language::Java),
        Some("cs") => Ok(Language::CSharp),
        Some("rb") => Ok(Language::Ruby),
        other => Err(format!(
            "extensão de arquivo não reconhecida ({other:?}); use .java, .cs ou .rb"
        )),
    }
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let path = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("uso: static-analyzer <arquivo.java|arquivo.cs|arquivo.rb> [--json]");
            return ExitCode::FAILURE;
        }
    };
    let json_output = args.any(|a| a == "--json");

    let language = match detect_language(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let source = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("erro lendo '{path}': {e}");
            return ExitCode::FAILURE;
        }
    };

    let parse_result = match language {
        Language::Java => java_adapter::parse_java(&source, &path),
        Language::CSharp => csharp_adapter::parse_csharp(&source, &path),
        Language::Ruby => ruby_adapter::parse_ruby(&source, &path),
    };

    let complexity_ir = match parse_result {
        Ok(ir) => ir,
        Err(e) => {
            eprintln!("erro analisando '{path}': {e}");
            return ExitCode::FAILURE;
        }
    };

    let results = engine::analyze(&complexity_ir);

    if json_output {
        println!("{}", serde_json::to_string_pretty(&results).unwrap());
        return ExitCode::SUCCESS;
    }

    println!("Análise estática: {path}\n");
    if results.is_empty() {
        println!("(nenhum método encontrado)");
    }
    for r in &results {
        println!("método '{}' (linha {})", r.method_name, r.line);
        println!("  Time:  {}", r.time);
        println!("  Space: {}", r.space);
        if !r.evidence.is_empty() {
            println!("  Evidências:");
            for e in &r.evidence {
                println!("    - {e}");
            }
        }
        println!();
    }

    ExitCode::SUCCESS
}
