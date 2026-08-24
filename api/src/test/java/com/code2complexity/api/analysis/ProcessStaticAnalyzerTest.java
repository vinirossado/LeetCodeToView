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

    @Test
    void rejectsUnsupportedLanguage() {
        StaticAnalyzer.UnsupportedLanguageException ex = assertThrows(
                StaticAnalyzer.UnsupportedLanguageException.class,
                () -> newAnalyzer().analyze("ruby", "puts 1"));

        assertTrue(ex.getMessage().contains("ruby"));
    }
}
