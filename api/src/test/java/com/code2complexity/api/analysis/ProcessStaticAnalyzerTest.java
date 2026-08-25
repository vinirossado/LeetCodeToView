package com.code2complexity.api.analysis;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.fasterxml.jackson.databind.JsonNode;
import java.lang.reflect.Field;
import org.junit.jupiter.api.Test;

// Plain unit test (no @QuarkusTest, no CDI) — calls the real static-analyzer
// binary as a subprocess, same discipline used to validate the sandbox
// binaries elsewhere in this project (real execution, not just design).
// Assumes `cargo build --release` has been run in static-analyzer/, same
// precondition as running the API itself against real binaries.
class ProcessStaticAnalyzerTest {

    private ProcessStaticAnalyzer newAnalyzer() {
        ProcessStaticAnalyzer analyzer = new ProcessStaticAnalyzer();
        try {
            Field field = ProcessStaticAnalyzer.class.getDeclaredField("binaryPath");
            field.setAccessible(true);
            field.set(analyzer, "../static-analyzer/target/release/static-analyzer");
        } catch (ReflectiveOperationException e) {
            throw new RuntimeException(e);
        }
        return analyzer;
    }

    @Test
    void analyzesJavaCode() throws Exception {
        JsonNode result = newAnalyzer().analyze("java", """
                class Main {
                    void m(int[] a) {
                        for (int i = 0; i < a.length; i++) {
                            System.out.println(i);
                        }
                    }
                }
                """);

        assertEquals(1, result.size());
        assertEquals("Linear", result.get(0).get("time").asText());
    }

    @Test
    void analyzesCsharpCode() throws Exception {
        JsonNode result = newAnalyzer().analyze("csharp", """
                for (int i = 0; i < 10; i++) {
                    Console.WriteLine(i);
                }
                """);

        assertEquals(1, result.size());
        assertEquals("Linear", result.get(0).get("time").asText());
    }

    // Added alongside the "ruby" -> "rb" EXTENSIONS entry (see
    // ProcessStaticAnalyzer): before that entry existed, "ruby" was this
    // test class's own stand-in for "an unsupported language" (see
    // rejectsUnsupportedLanguage below, which used to pass `"ruby"` before
    // this task made it a genuinely supported one and had to be moved to
    // "python" instead) — this test now covers the real, newly-wired path
    // static-analyzer/src/ruby_adapter.rs already implemented and validated
    // on its own terms (see tasks.md "Adaptador AST (Ruby)"), same shape as
    // analyzesJavaCode/analyzesCsharpCode above.
    @Test
    void analyzesRubyCode() throws Exception {
        // Wrapped in a `def`, not top-level statements — ruby_adapter.rs
        // deliberately does NOT classify top-level code the way the C#
        // adapter does for top-level-statement programs (see tasks.md
        // "Decisão consciente de NÃO replicar o suporte a 'top-level
        // statements' do C#"): a bare `arr.each { ... }` with no enclosing
        // method produces zero method entries, found the hard way here
        // first (assertEquals(1, ...) failed with 0 before adding the
        // `def`).
        JsonNode result = newAnalyzer().analyze("ruby", """
                def sum(arr)
                    total = 0
                    arr.each { |x| total += x }
                    total
                end
                """);

        assertEquals(1, result.size());
        assertEquals("Linear", result.get(0).get("time").asText());
    }

    @Test
    void rejectsUnsupportedLanguage() {
        // "python" here, not "ruby" — "ruby" used to be this test's
        // stand-in for "unsupported" before this task wired it up for
        // real (see analyzesRubyCode above and ProcessStaticAnalyzer's
        // EXTENSIONS map) — found breaking exactly here when this task's
        // change made this assertion start failing for a real reason
        // ("nothing was thrown"), not a false positive to paper over.
        StaticAnalyzer.UnsupportedLanguageException ex = assertThrows(
                StaticAnalyzer.UnsupportedLanguageException.class,
                () -> newAnalyzer().analyze("python", "print(1)"));

        assertTrue(ex.getMessage().contains("python"));
    }
}
