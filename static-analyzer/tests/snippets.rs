//! Regression test: runs the built `static-analyzer` binary against every file
//! under `test-snippets/` (Java) and `test-snippets-csharp/` (C#), the same way
//! this crate was validated manually throughout the project so far, and asserts
//! the CLI still behaves like a well-formed analyzer: exits successfully and
//! prints a JSON array on stdout when invoked with `--json`.
//!
//! This intentionally does NOT assert exact Big-O classifications per snippet —
//! the engine's heuristics (see `src/engine.rs`) are expected to evolve, and
//! pinning exact output here would make this test brittle rather than useful.
//! What it does catch: a crash, a panic, a non-zero exit, or malformed JSON on
//! any of the real snippets this project has already validated by hand — i.e.
//! basic build/CLI-contract regressions, wired into CI (unlike the manual
//! `cargo run -- file --json` checks used during development).

use std::path::Path;
use std::process::Command;

fn run_on_snippet(path: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_static-analyzer"))
        .arg(path)
        .arg("--json")
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn static-analyzer for {path:?}: {e}"));

    assert!(
        output.status.success(),
        "static-analyzer exited non-zero for {path:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|e| panic!("non-utf8 stdout for {path:?}: {e}"));

    let parsed: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout for {path:?} was not valid JSON: {e}\n{stdout}"));

    assert!(
        parsed.is_array(),
        "expected top-level JSON array for {path:?}, got: {parsed}"
    );
}

fn run_on_all_snippets_in(dir: &str) {
    let dir_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
    let entries: Vec<_> = std::fs::read_dir(&dir_path)
        .unwrap_or_else(|e| panic!("could not read snippet dir {dir_path:?}: {e}"))
        .map(|e| e.expect("readdir entry").path())
        .collect();

    assert!(
        !entries.is_empty(),
        "expected at least one snippet in {dir_path:?}"
    );

    for path in entries {
        run_on_snippet(&path);
    }
}

#[test]
fn java_snippets_produce_valid_json() {
    run_on_all_snippets_in("test-snippets");
}

#[test]
fn csharp_snippets_produce_valid_json() {
    run_on_all_snippets_in("test-snippets-csharp");
}
