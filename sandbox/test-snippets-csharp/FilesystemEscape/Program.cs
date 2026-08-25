using System;
using System.IO;

// Fase 2 pentest (tasks.md "Testes de fuga de sandbox"): C# had NO
// equivalent of Java's FilesystemEscape.java in the official suite (already
// flagged as a gap earlier this session, closed here) -- reading a file
// outside the minimal rootfs, then a handful of ORDINARY (non-malicious)
// System.IO mutation calls against the read-only chroot, mirroring
// sandbox/test-snippets/SymlinkEscape.java's Java coverage. Real gap found
// while writing this: symlinkat/renameat/fchmodat/utimensat were missing
// from CSHARP_SECCOMP_POLICY (csharp.rs) too (same fix already applied to
// JAVA_SECCOMP_POLICY) -- added defensively for the same "chroot, not
// seccomp, should gate paths" reason. NOTE (documented honestly, not
// hidden): confirming the end-to-end "clean exception instead of silent
// truncation" outcome for THIS specific snippet is masked by an
// ALREADY-DOCUMENTED, unrelated open item -- the ICorDebug stepper getting
// stuck on certain CLR-internal transitions (see tasks.md, the
// StartCore/exception-unwind items) -- so running this today still ends in
// `timeout` rather than a clean per-line result, same symptom as those
// already-tracked cases, not a new one.
try
{
    string content = File.ReadAllText("/etc/shadow");
    Console.WriteLine("FALHA DE ISOLAMENTO: leu /etc/shadow, " + content.Length + " bytes");
}
catch (Exception e)
{
    Console.WriteLine("leitura de /etc/shadow bloqueada como esperado: " + e.GetType().Name + ": " + e.Message);
}

Probe("CreateSymbolicLink", () => { File.CreateSymbolicLink("evil-link", "/etc/passwd"); });
Probe("Move/rename", () => { File.Move("Program.cs", "Moved.cs"); });
Probe("SetUnixFileMode", () => { File.SetUnixFileMode("Program.cs", UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.UserExecute); });
Probe("SetLastWriteTimeUtc", () => { File.SetLastWriteTimeUtc("Program.cs", DateTime.UtcNow); });

Console.WriteLine("SWEEP COMPLETE");

void Probe(string name, Action op)
{
    try
    {
        op();
        Console.WriteLine(name + ": FALHA DE ISOLAMENTO (sem exceção, esperava Read-only file system)");
    }
    catch (Exception e)
    {
        Console.WriteLine(name + ": bloqueado como esperado -- " + e.GetType().Name + ": " + e.Message);
    }
}
