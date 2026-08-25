using System;
using System.Net;
using System.Net.Sockets;

// Fase 2 pentest (tasks.md "Testes de fuga de sandbox"): C# had NO
// equivalent of Java's NetworkEscape.java in the official suite (already
// flagged as a gap earlier this session, closed here). ALREADY-DOCUMENTED,
// NOT a new finding: a raw synchronous Socket.Connect in C# is expected to
// be blocked by this sandbox's own pre-existing multi-thread guard (>2
// distinct ICorDebugThread pointers = blocked, MVP scope, see com.rs)
// BEFORE any real connect() syscall happens -- .NET's socket layer lazily
// spins up an internal epoll-readiness thread on first socket use, which
// trips that guard first. Kept here as a permanent regression check (not
// just a throwaway probe) so a future change to either the multi-thread
// guard or CSHARP_SECCOMP_POLICY's socket syscalls doesn't silently change
// this without a test noticing.
try
{
    using var s = new Socket(AddressFamily.InterNetwork, SocketType.Stream, ProtocolType.Tcp);
    s.Connect(new IPEndPoint(IPAddress.Parse("8.8.8.8"), 53));
    Console.WriteLine("FALHA DE ISOLAMENTO: conexão de rede funcionou");
}
catch (Exception e)
{
    Console.WriteLine("rede bloqueada (ou execução bloqueada pelo guard de multi-thread, ver com.rs): " + e.GetType().Name + ": " + e.Message);
}
