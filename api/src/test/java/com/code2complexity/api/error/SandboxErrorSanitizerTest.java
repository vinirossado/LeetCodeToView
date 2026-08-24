package com.code2complexity.api.error;

import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.code2complexity.api.model.Execution;
import com.code2complexity.api.sandbox.ProcessSandboxRunner;
import java.io.IOException;
import java.lang.reflect.Field;
import org.junit.jupiter.api.Test;

// Plain unit test (no @QuarkusTest, no CDI) — same discipline as
// ProcessStaticAnalyzerTest: the Java/C# compile-error cases run the real
// javac/dotnet toolchain (through the real ProcessSandboxRunner) rather
// than guessing at their output shape, so a change in the real diagnostic
// format would actually break this test instead of leaving a stale fake.
class SandboxErrorSanitizerTest {

    private ProcessSandboxRunner newRunner() {
        ProcessSandboxRunner runner = new ProcessSandboxRunner();
        try {
            Field field = ProcessSandboxRunner.class.getDeclaredField("binaryPath");
            field.setAccessible(true);
            field.set(runner, "../sandbox/target/release/sandbox-runner");
        } catch (ReflectiveOperationException e) {
            throw new RuntimeException(e);
        }
        return runner;
    }

    @Test
    void realJavaCompileErrorPassesThroughWithPathsStripped() {
        // Missing semicolon — guaranteed javac diagnostic ("cannot find
        // symbol" or "';' expected", depending on javac's own recovery),
        // not a sandbox-internal failure.
        Execution execution = new Execution("exec-1", "java", """
                class Main {
                    public static void main(String[] args) {
                        int x = 1
                    }
                }
                """);

        IOException failure = assertThrows(IOException.class,
                () -> newRunner().run(execution, line -> { }));

        String sanitized = SandboxErrorSanitizer.sanitize(failure.getMessage(), "test");

        assertTrue(sanitized.contains("error"), "expected javac's own diagnostic to survive: " + sanitized);
        assertFalse(sanitized.contains("/var/tmp/"), "must not leak the host temp workdir: " + sanitized);
        assertFalse(sanitized.contains(SandboxErrorSanitizer.GENERIC_MESSAGE),
                "a real compiler diagnostic must not be replaced by the generic message");
    }

    @Test
    void realCsharpCompileErrorPassesThroughWithPathsStripped() {
        // ProcessSandboxRunner#compileCsharp runs `dotnet build` before
        // ever invoking sandbox-runner, so this doesn't depend on the
        // sandbox-runner binary at all — only on `dotnet` being installed
        // (it is, see application.properties / task preconditions).
        Execution execution = new Execution("exec-2", "csharp", "Console.WriteLine(1 + ;");

        IOException failure = assertThrows(IOException.class,
                () -> newRunner().run(execution, line -> { }));
        assertTrue(failure.getMessage().startsWith("C# compilation failed:"));

        String sanitized = SandboxErrorSanitizer.sanitize(failure.getMessage(), "test");

        assertTrue(sanitized.contains("error CS") || sanitized.toLowerCase().contains("error"),
                "expected dotnet build's own diagnostic to survive: " + sanitized);
        assertFalse(sanitized.contains("/var/tmp/"), "must not leak the host temp workdir: " + sanitized);
        assertFalse(sanitized.contains(SandboxErrorSanitizer.GENERIC_MESSAGE),
                "a real compiler diagnostic must not be replaced by the generic message");
    }

    @Test
    void simulatedInternalFailureBecomesGenericMessage() {
        // Shaped like a real observed sandbox-runner failure (see
        // tasks.md's smoke-test entry: "panicked at src/java.rs:80:10:
        // falha ao rodar nsjail...") — no javac/dotnet compile marker in
        // it, so this must NOT be treated as a compiler diagnostic.
        String rawMessage = "sandbox-runner exited with code 137: "
                + "[sandbox-runner/java] compilando \"/var/tmp/code2complexity-91653f51-abcd-1234/Main.java\"...\n"
                + "thread 'main' panicked at src/java.rs:80:10: falha ao rodar nsjail: "
                + "No such file or directory (os error 2)";

        String sanitized = SandboxErrorSanitizer.sanitize(rawMessage, "test");

        assertTrue(sanitized.equals(SandboxErrorSanitizer.GENERIC_MESSAGE));
        assertFalse(sanitized.toLowerCase().contains("nsjail"));
        assertFalse(sanitized.contains("panicked"));
        assertFalse(sanitized.contains("java.rs"));
        assertFalse(sanitized.contains("/var/tmp/"));
    }

    @Test
    void nullMessageBecomesGenericMessage() {
        assertTrue(SandboxErrorSanitizer.sanitize(null, "test").equals(SandboxErrorSanitizer.GENERIC_MESSAGE));
    }
}
