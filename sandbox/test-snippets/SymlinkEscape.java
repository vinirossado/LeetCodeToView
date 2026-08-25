import java.nio.file.*;
import java.nio.file.attribute.*;

// Fase 2 pentest (tasks.md "Testes de fuga de sandbox"): tries a handful of
// java.nio.file.Files mutating operations that are ORDINARY, non-malicious
// Java NIO usage (not creative escape tricks) against the read-only chroot.
// Real gap found and fixed while writing this: symlinkat/linkat/renameat/
// fchmodat/utimensat were missing from JAVA_SECCOMP_POLICY (java.rs) --
// none of the pre-existing test-snippets happen to call them, so the
// original strace-derived allowlist never included them. Missing them meant
// any of these calls killed the target JVM via an uncatchable SIGSYS
// instead of the intended, catchable filesystem exception -- and the
// jdi/Debugger.java driver reported this as a normal exit(0), i.e. the
// execution silently truncated at that exact line and the API would have
// shown `status: "completed"`, no different from the general "uncaught
// exception silently reported as completed" bug already fixed once this
// session. See tasks.md for the full empirical trail (strace, the A/B
// control against the openat-removal case).
public class SymlinkEscape {
    public static void main(String[] args) {
        probe("createSymbolicLink", () -> Files.createSymbolicLink(Paths.get("evil-link"), Paths.get("/etc/passwd")));
        probe("createLink (hardlink)", () -> Files.createLink(Paths.get("evil-hardlink"), Paths.get("SymlinkEscape.java")));
        probe("move/rename", () -> Files.move(Paths.get("SymlinkEscape.java"), Paths.get("Moved.java")));
        probe("setPosixFilePermissions", () -> Files.setPosixFilePermissions(Paths.get("SymlinkEscape.java"), PosixFilePermissions.fromString("rwxrwxrwx")));
        probe("setLastModifiedTime", () -> Files.setLastModifiedTime(Paths.get("SymlinkEscape.java"), FileTime.fromMillis(0)));
        System.out.println("SWEEP COMPLETE (esperado: todas as 5 tentativas acima terminam em exceção capturada, nunca em SIGSYS/truncamento silencioso)");
    }

    interface Op {
        void run() throws Exception;
    }

    static void probe(String name, Op op) {
        try {
            op.run();
            System.out.println(name + ": FALHA DE ISOLAMENTO (sem exceção, esperava Read-only file system)");
        } catch (Exception e) {
            System.out.println(name + ": bloqueado como esperado -- " + e);
        }
    }
}
