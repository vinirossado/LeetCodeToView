// Runtime C# (ICorDebug via interop direto — ver com/). Diferente do Java
// (onde nsjail lança um processo Java separado que fala JDWP com o driver),
// aqui a lógica de debug roda DENTRO do nosso próprio binário Rust — porque
// dbgshim precisa manipular o processo debuggee diretamente via FFI.
//
// Por isso o padrão é "self re-exec": o processo externo (outer::run_outer)
// faz fork+exec do nsjail apontando pra ELE MESMO de novo, mas com uma flag
// interna (CSHARP_WORKER_FLAG) que faz o binário, já dentro do jail, pular
// direto pra worker::run_worker() em vez de tentar reabrir o nsjail
// recursivamente.
//
// Flags de nsjail já validadas empiricamente no spike (não inventar de novo):
//   --rlimit_fsize inf       (CoreCLR faz memfd_create("doublemapper") +
//                              ftruncate pra 2TB — reserva virtual, não disco
//                              real; sem isso o processo morre com SIGXFSZ)
//   --tmpfsmount /tmp        (o handshake de debug do CoreCLR cria socket/pipes
//                              em /tmp — se for read-only, EROFS e trava)
//   DOTNET_GCHeapHardLimit   (sem isso o CoreCLR tenta dimensionar o heap pela
//                              RAM total do host e estoura o rlimit_as)
//
// Module layout: `seccomp` holds just the (long) seccomp-bpf policy string,
// `outer` holds run_outer (the un-jailed dispatcher-side half) plus
// RunOptions, and `worker` holds run_worker (the jailed, dbgshim/ICorDebug-
// driving half). Re-exported below so external call sites
// (bin/sandbox_runner.rs) keep using `csharp::run_outer`/`csharp::run_worker`/
// `csharp::RunOptions`/`csharp::CSHARP_WORKER_FLAG` unchanged.

mod outer;
mod seccomp;
mod worker;

pub use outer::{run_outer, RunOptions};
pub use worker::run_worker;

pub const CSHARP_WORKER_FLAG: &str = "--csharp-worker";
