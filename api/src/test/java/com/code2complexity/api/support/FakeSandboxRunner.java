package com.code2complexity.api.support;

import com.code2complexity.api.model.Execution;
import com.code2complexity.api.sandbox.SandboxRunner;
import jakarta.annotation.Priority;
import jakarta.enterprise.context.ApplicationScoped;
import jakarta.enterprise.inject.Alternative;
import java.util.ArrayList;
import java.util.List;
import java.util.function.Consumer;

/**
 * Fake runner used in tests: never touches nsjail/Docker/the real binary.
 * Lets the test control exactly which JSON lines the "sandbox" produces.
 * Activated in place of {@link com.code2complexity.api.sandbox.ProcessSandboxRunner}
 * via {@code quarkus.arc.selected-alternatives} (src/test/resources/application.properties).
 */
@Alternative
@Priority(1)
@ApplicationScoped
public class FakeSandboxRunner implements SandboxRunner {

    public record RunCall(String language, String code) {
    }

    // CDI-managed @ApplicationScoped beans are accessed through a client
    // proxy: method calls delegate to the shared instance, but a direct
    // public-field read/write on the proxy only touches the proxy's own
    // field slot, never the delegate. So state has to go through
    // getters/setters here, not bare fields, or callers with their own
    // injected reference (like this test's own @Inject) would silently
    // read/write a value the real instance never sees.
    private volatile List<String> lines = new ArrayList<>();
    private volatile Exception error;
    private final List<RunCall> runCalls = new ArrayList<>();

    @Override
    public void run(Execution execution, Consumer<String> onLine) throws Exception {
        runCalls.add(new RunCall(execution.getLanguage(), execution.getCode()));
        if (error != null) {
            throw error;
        }
        for (String line : lines) {
            onLine.accept(line);
        }
    }

    public void setLines(List<String> lines) {
        this.lines = lines;
    }

    public void setError(Exception error) {
        this.error = error;
    }

    public List<RunCall> getRunCalls() {
        return runCalls;
    }

    public void reset() {
        lines = new ArrayList<>();
        error = null;
        runCalls.clear();
    }
}
