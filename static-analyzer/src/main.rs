mod engine;
mod ir;
mod java_adapter;

use std::env;
use std::fs;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let path = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("uso: static-analyzer <arquivo.java> [--json]");
            return ExitCode::FAILURE;
        }
    };
    let json_output = args.any(|a| a == "--json");

    let source = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("erro lendo '{path}': {e}");
            return ExitCode::FAILURE;
        }
    };

    let complexity_ir = match java_adapter::parse_java(&source, &path) {
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
