package com.code2complexity.api.sandbox;

import com.code2complexity.api.model.Execution;
import jakarta.enterprise.context.ApplicationScoped;
import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.UUID;
import java.util.function.Consumer;
import org.eclipse.microprofile.config.inject.ConfigProperty;

/**
 * Real implementation: prepares the submitted source (compiling it first
 * for C#, see below) and shells out to the {@code sandbox-runner} Rust
 * binary (which itself wraps nsjail — no docker.sock, no daemon, see
 * spec.md "Isolamento").
 */
@ApplicationScoped
public class ProcessSandboxRunner implements SandboxRunner {

    // MUST NOT be under /tmp. csharp.rs's nsjail invocation mounts a fresh
    // empty tmpfs at /tmp inside the jail (--tmpfsmount /tmp, needed for
    // CoreCLR's diagnostic IPC socket — see spec.md "Estratégia C#"), which
    // shadows/hides anything the API wrote there beforehand: a .dll built
    // under the JVM's default temp dir (java.io.tmpdir, itself /tmp on
    // Linux) becomes invisible to the jailed child, and nsjail fails with
    // "chdir(...): No such file or directory" — reproduced empirically
    // running the real Docker image. /var/tmp is outside that remount and
    // was already the established workaround elsewhere in this project's
    // Fase 0.5 spike testing, for the same underlying reason.
    private static final Path WORK_DIR_ROOT = Path.of("/var/tmp");

    // Mirrors the settings already validated against sandbox-runner's C#
    // path (sandbox/test-snippets-csharp/*/*.csproj) — same TFM, same
    // flags. Framework-dependent build (no <SelfContained>), because
    // sandbox-runner launches it via `dotnet <dll>` (see csharp.rs,
    // cmdline = "/usr/share/dotnet/dotnet {dll}"), not as a standalone apphost.
    private static final String CSPROJ = """
            <Project Sdk="Microsoft.NET.Sdk">
              <PropertyGroup>
                <OutputType>Exe</OutputType>
                <TargetFramework>net8.0</TargetFramework>
                <ImplicitUsings>enable</ImplicitUsings>
                <Nullable>enable</Nullable>
                <InvariantGlobalization>true</InvariantGlobalization>
              </PropertyGroup>
            </Project>
            """;

    @ConfigProperty(name = "sandbox.runner.binary-path")
    String binaryPath;

    @Override
    public void run(Execution execution, Consumer<String> onLine) throws IOException, InterruptedException {
        Files.createDirectories(WORK_DIR_ROOT);
        Path workDir = Files.createTempDirectory(WORK_DIR_ROOT, "code2complexity-" + UUID.randomUUID());
        try {
            Path fileToRun = switch (execution.getLanguage()) {
                case "java" -> writeJavaSource(workDir, execution.getCode());
                case "csharp" -> compileCsharp(workDir, execution.getCode());
                case "ruby" -> writeRubySource(workDir, execution.getCode());
                default -> throw new IllegalArgumentException("unsupported language: " + execution.getLanguage());
            };

            runSandboxRunnerBinary(execution.getLanguage(), fileToRun, onLine);
        } finally {
            deleteRecursively(workDir);
        }
    }

    // javac requires the file name to match the public class name, so for
    // the MVP we require submitted Java code to declare `public class
    // Main`. Not yet enforced/validated on the way in — another open
    // piece of the API<->Sandbox Controller contract (#153).
    private static Path writeJavaSource(Path workDir, String code) throws IOException {
        Path sourcePath = workDir.resolve("Main.java");
        Files.writeString(sourcePath, code, StandardCharsets.UTF_8);
        return sourcePath;
    }

    // No compile step, no naming constraint, unlike Java (writeJavaSource
    // above) and even C# (which at least needs a real .csproj/.cs pair for
    // `dotnet build`) — sandbox/ruby/driver.rb just `load`s whatever file
    // ruby.rs passes it. "main.rb" is an arbitrary-but-fixed name (any name
    // would do; ruby.rs discovers it by passing the same PathBuf's file
    // name through unchanged, mirroring how writeJavaSource always writes
    // "Main.java" — a fixed name here is simpler than deriving one from the
    // submitted code, and there is nothing in Ruby's `load` semantics that
    // would benefit from a variable one the way Java's javac does).
    private static Path writeRubySource(Path workDir, String code) throws IOException {
        Path sourcePath = workDir.resolve("main.rb");
        Files.writeString(sourcePath, code, StandardCharsets.UTF_8);
        return sourcePath;
    }

    // Unlike Java, C# top-level statements don't require any particular
    // class/file name (sandbox-runner's ICorDebug entry-point search
    // already handles both top-level-statement and explicit Main() code,
    // see tasks.md "busca robusta de token"), so there's no naming
    // constraint to document here.
    private Path compileCsharp(Path workDir, String code) throws IOException, InterruptedException {
        Files.writeString(workDir.resolve("app.csproj"), CSPROJ, StandardCharsets.UTF_8);
        Files.writeString(workDir.resolve("Program.cs"), code, StandardCharsets.UTF_8);

        Path outputDir = workDir.resolve("out");
        Process build = new ProcessBuilder("dotnet", "build", "-c", "Debug", "-o", outputDir.toString())
                .directory(workDir.toFile())
                .redirectErrorStream(true)
                .start();

        String buildOutput;
        try (BufferedReader reader = new BufferedReader(new InputStreamReader(build.getInputStream(), StandardCharsets.UTF_8))) {
            buildOutput = reader.lines().collect(java.util.stream.Collectors.joining("\n"));
        }
        int exitCode = build.waitFor();
        if (exitCode != 0) {
            throw new IOException("C# compilation failed:\n" + buildOutput);
        }

        return outputDir.resolve("app.dll");
    }

    private void runSandboxRunnerBinary(String language, Path file, Consumer<String> onLine) throws IOException, InterruptedException {
        Process process = new ProcessBuilder(binaryPath, "--language", language, "--file", file.toString()).start();

        // sandbox-runner logs progress/diagnostics on stderr (see the
        // `eprintln!` calls throughout java.rs/csharp.rs). It has to be
        // drained on its own thread concurrently with stdout, or a
        // process that writes enough of it deadlocks once the OS pipe
        // buffer fills up and nobody is reading it.
        StringBuilder stderr = new StringBuilder();
        Thread stderrReader = Thread.ofVirtual().start(() -> {
            try (BufferedReader reader = new BufferedReader(new InputStreamReader(process.getErrorStream(), StandardCharsets.UTF_8))) {
                String line;
                while ((line = reader.readLine()) != null) {
                    stderr.append(line).append('\n');
                }
            } catch (IOException e) {
                // process died / stream closed — nothing left to drain
            }
        });

        try (BufferedReader reader = new BufferedReader(new InputStreamReader(process.getInputStream(), StandardCharsets.UTF_8))) {
            String line;
            while ((line = reader.readLine()) != null) {
                onLine.accept(line);
            }
        }
        int exitCode = process.waitFor();
        stderrReader.join();

        if (exitCode != 0) {
            throw new IOException("sandbox-runner exited with code " + exitCode + ": " + stderr);
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
