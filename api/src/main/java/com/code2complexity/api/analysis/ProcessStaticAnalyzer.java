package com.code2complexity.api.analysis;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import jakarta.enterprise.context.ApplicationScoped;
import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;
import java.util.UUID;
import org.eclipse.microprofile.config.inject.ConfigProperty;

/**
 * Real implementation: writes the source to a temp file and shells out to
 * the {@code static-analyzer} binary with {@code --json}. Synchronous —
 * tree-sitter parsing a small user snippet is near-instant, and unlike
 * {@link com.code2complexity.api.sandbox.ProcessSandboxRunner} there's no
 * untrusted code actually running, so no virtual-thread/streaming dance is
 * needed here.
 */
@ApplicationScoped
public class ProcessStaticAnalyzer implements StaticAnalyzer {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    // static-analyzer's CLI picks the adapter from the file extension (see
    // static-analyzer/src/main.rs) — not from a separate flag.
    private static final Map<String, String> EXTENSIONS = Map.of("java", "java", "csharp", "cs", "ruby", "rb");

    // Same /var/tmp reasoning as ProcessSandboxRunner: harmless here since
    // static-analyzer never runs inside nsjail, but kept consistent so
    // this class doesn't become the one place that silently breaks if a
    // future change ever routes it through the sandbox too.
    private static final Path WORK_DIR_ROOT = Path.of("/var/tmp");

    @ConfigProperty(name = "static-analyzer.binary-path")
    String binaryPath;

    @Override
    public JsonNode analyze(String language, String code) throws IOException, InterruptedException {
        String extension = EXTENSIONS.get(language);
        if (extension == null) {
            throw new UnsupportedLanguageException(language);
        }

        Files.createDirectories(WORK_DIR_ROOT);
        Path workDir = Files.createTempDirectory(WORK_DIR_ROOT, "code2complexity-analysis-" + UUID.randomUUID());
        try {
            Path sourcePath = workDir.resolve("Main." + extension);
            Files.writeString(sourcePath, code, StandardCharsets.UTF_8);

            Process process = new ProcessBuilder(binaryPath, sourcePath.toString(), "--json")
                    .redirectErrorStream(true)
                    .start();

            String output;
            try (BufferedReader reader = new BufferedReader(new InputStreamReader(process.getInputStream(), StandardCharsets.UTF_8))) {
                output = reader.lines().collect(java.util.stream.Collectors.joining("\n"));
            }
            int exitCode = process.waitFor();
            if (exitCode != 0) {
                throw new IOException("static-analyzer exited with code " + exitCode + ": " + output);
            }

            return MAPPER.readTree(output);
        } finally {
            deleteRecursively(workDir);
        }
    }

    private static void deleteRecursively(Path root) throws IOException {
        if (!Files.exists(root)) {
            return;
        }
        try (var paths = Files.walk(root)) {
            paths.sorted((a, b) -> b.compareTo(a)).forEach(path -> {
                try {
                    Files.delete(path);
                } catch (IOException e) {
                    throw new RuntimeException(e);
                }
            });
        }
    }
}
