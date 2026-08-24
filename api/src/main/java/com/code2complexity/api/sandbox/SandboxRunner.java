package com.code2complexity.api.sandbox;

import com.code2complexity.api.model.Execution;
import java.util.function.Consumer;

/**
 * Abstraction over "run code inside the sandbox". The real implementation
 * ({@link ProcessSandboxRunner}) forks+execs the {@code sandbox-runner}
 * binary (Rust, which in turn calls nsjail); tests swap in a fake so they
 * don't depend on Docker/nsjail/JVM/CoreCLR being installed.
 *
 * <p>{@code run} blocks until the execution finishes, calling
 * {@code onLine} once per emitted JSON event line (same schema as
 * sandbox/src/events.rs), in the order they arrive.
 */
public interface SandboxRunner {
    void run(Execution execution, Consumer<String> onLine) throws Exception;
}
