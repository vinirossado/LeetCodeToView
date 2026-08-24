package com.code2complexity.api.analysis;

import com.fasterxml.jackson.databind.JsonNode;

/**
 * Abstraction over "run static complexity analysis on a piece of source
 * code". The real implementation ({@link ProcessStaticAnalyzer}) shells
 * out to the {@code static-analyzer} Rust binary (tree-sitter based, no
 * code execution involved — unlike {@link com.code2complexity.api.sandbox.SandboxRunner},
 * this never runs untrusted code, only parses it); tests swap in a fake.
 *
 * @throws UnsupportedLanguageException if the analyzer has no adapter for
 *     the given language yet (currently: anything other than "java" — see
 *     tasks.md "Static Analyzer", C# adapter not started)
 */
public interface StaticAnalyzer {
    JsonNode analyze(String language, String code) throws Exception;

    class UnsupportedLanguageException extends RuntimeException {
        public UnsupportedLanguageException(String language) {
            super("static analysis not implemented yet for language: " + language);
        }
    }
}
