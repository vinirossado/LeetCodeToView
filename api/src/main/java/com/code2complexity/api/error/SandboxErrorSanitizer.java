package com.code2complexity.api.error;

import io.quarkus.logging.Log;
import java.util.regex.Pattern;

/**
 * Sanitizes runtime/subprocess failure messages before they reach the
 * frontend (see spec.md "Segurança" and tasks.md Fase 2, "Sanitizar
 * mensagens de erro/stack trace do runtime").
 *
 * <p>Raw failures coming out of {@code sandbox-runner}, {@code dotnet
 * build}, or {@code static-analyzer} can leak absolute host paths (the
 * per-execution temp workdir under {@code /var/tmp}), Rust panic locations
 * (e.g. {@code src/java.rs:80:10}), or other sandbox-internal details a
 * user has no business seeing. A genuine compiler diagnostic (javac /
 * {@code dotnet build}), on the other hand, is exactly what the user needs
 * to fix their own code, so it is detected and passed through — with any
 * absolute temp-workdir path stripped — instead of being replaced by the
 * generic message below.
 */
public final class SandboxErrorSanitizer {

    public static final String GENERIC_MESSAGE = "execution failed due to an internal sandbox error";

    // sandbox/src/java.rs prints this exact marker on its own stderr right
    // after a non-zero javac exit code (`eprintln!("[sandbox-runner/java]
    // falha na compilação")`). javac itself inherits sandbox-runner's
    // stdio, so its real diagnostic ends up interleaved on that same
    // stream — ProcessSandboxRunner#runSandboxRunnerBinary folds all of it
    // into the IOException it throws on a non-zero exit code.
    private static final String JAVA_COMPILE_MARKER = "[sandbox-runner/java] falha na compilação";

    // Printed just before javac runs (sandbox/src/java.rs:
    // `eprintln!("[sandbox-runner/java] compilando {java_file:?}...")`).
    // Used to find where sandbox-runner's own log line ends and javac's
    // real diagnostic begins.
    private static final String JAVA_COMPILE_HEADER_SUFFIX = "...\n";

    // ProcessSandboxRunner#compileCsharp throws this IOException directly
    // (its own message, built from `dotnet build`'s captured output) —
    // the exception's own text already unambiguously identifies this as a
    // compiler diagnostic, no marker-sniffing of a subprocess's stderr
    // needed like the Java case above.
    private static final String CSHARP_COMPILE_PREFIX = "C# compilation failed:";

    // Matches the per-execution/per-analysis temp workdir created under
    // /var/tmp (ProcessSandboxRunner.WORK_DIR_ROOT /
    // ProcessStaticAnalyzer.WORK_DIR_ROOT), e.g.
    // "/var/tmp/code2complexity-91653f51-.../" or
    // "/var/tmp/code2complexity-analysis-<uuid>/" — stripped even from an
    // otherwise-legitimate compiler diagnostic so absolute host paths never
    // reach the client. The leading "\S*" (no whitespace) also absorbs any
    // path prefix some tools resolve /var/tmp through — confirmed with
    // `dotnet build`, which prints "/private/var/tmp/..." on a host where
    // /var is itself a symlink to /private/var (macOS); on a real Linux
    // container (the actual deploy target) there is no such prefix, but
    // stripping it defensively either way costs nothing.
    private static final Pattern WORKDIR_PATH = Pattern.compile("\\S*/var/tmp/code2complexity[\\w-]*/?");

    private SandboxErrorSanitizer() {
    }

    /**
     * @param rawMessage the raw exception message (or subprocess output)
     *                    to sanitize.
     * @param context     short label identifying the failing component
     *                    (e.g. "execution", "static-analyzer"), used only
     *                    in the server-side log line — never sent to the
     *                    client.
     */
    public static String sanitize(String rawMessage, String context) {
        return sanitize(rawMessage, context, null);
    }

    /**
     * Same as {@link #sanitize(String, String)}, but when the message
     * doesn't match a known compiler diagnostic, the full {@code cause}
     * (with its Java-side stack trace, not just its message) is logged
     * server-side for debugging, instead of only the message text.
     */
    public static String sanitize(String rawMessage, String context, Throwable cause) {
        if (rawMessage == null || rawMessage.isBlank()) {
            logRawFailure(context, "failed with no error detail", cause);
            return GENERIC_MESSAGE;
        }

        if (rawMessage.startsWith(CSHARP_COMPILE_PREFIX)) {
            return stripWorkdirPaths(rawMessage);
        }

        int javaMarkerIndex = rawMessage.indexOf(JAVA_COMPILE_MARKER);
        if (javaMarkerIndex >= 0) {
            return stripWorkdirPaths(extractJavaDiagnostic(rawMessage, javaMarkerIndex));
        }

        // Anything else — nsjail failures, Rust panics, unexpected
        // non-zero exits, sandbox-runner/static-analyzer internal errors —
        // is not something the end user can act on, and may contain
        // sandbox internals. Log the full detail server-side for
        // debugging, but never forward it to the client.
        logRawFailure(context, "failed with an internal/unexpected error: " + rawMessage, cause);
        return GENERIC_MESSAGE;
    }

    private static void logRawFailure(String context, String detail, Throwable cause) {
        if (cause != null) {
            Log.warn(context + " " + detail + " (not exposing raw detail to client)", cause);
        } else {
            Log.warn(context + " " + detail + " (not exposing raw detail to client)");
        }
    }

    // Pulls just javac's own diagnostic text out of the raw message,
    // dropping sandbox-runner's own "compilando ...\n" / "falha na
    // compilação" log lines around it (and, incidentally, the
    // "sandbox-runner exited with code N: " wrapper prefix ProcessSandbox
    // Runner adds). Falls back to the full message if the expected header
    // isn't found, so a format change upstream degrades to "strip paths
    // from everything" rather than silently swallowing the diagnostic.
    private static String extractJavaDiagnostic(String rawMessage, int javaMarkerIndex) {
        int headerEnd = rawMessage.indexOf(JAVA_COMPILE_HEADER_SUFFIX);
        if (headerEnd < 0 || headerEnd + JAVA_COMPILE_HEADER_SUFFIX.length() > javaMarkerIndex) {
            return rawMessage;
        }
        String diagnostic = rawMessage.substring(headerEnd + JAVA_COMPILE_HEADER_SUFFIX.length(), javaMarkerIndex).strip();
        return diagnostic.isEmpty() ? rawMessage : diagnostic;
    }

    private static String stripWorkdirPaths(String message) {
        return WORKDIR_PATH.matcher(message).replaceAll("<workdir>/");
    }
}
