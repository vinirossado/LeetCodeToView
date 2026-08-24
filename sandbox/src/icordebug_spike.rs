mod com;

use std::env;
use std::os::raw::{c_int, c_void};
use std::ptr;
use std::time::{Duration, Instant};

use libloading::Library;

use com::{CorDebug, ManagedCallbackObj, IID_ICORDEBUG, MANAGED_CALLBACK_VTBL, S_OK};

// Spike: chama libdbgshim.so direto via FFI, sem passar pelo netcoredbg como
// processo externo. Objetivo: isolar se o handshake de baixo nível (CreateProcessForLaunch
// + RegisterForRuntimeStartup) funciona de forma confiável dentro do nsjail, já que o
// netcoredbg (via DAP) trava numa condição de corrida nesse ambiente.

type HResult = i32;
type Handle = *mut c_void;
type DWord = u32;
type WChar = u16;

type CreateProcessForLaunchFn = unsafe extern "C" fn(
    *mut WChar,   // lpCommandLine
    c_int,        // bSuspendProcess
    *mut c_void,  // lpEnvironment
    *const WChar, // lpCurrentDirectory
    *mut DWord,   // pProcessId
    *mut Handle,  // pResumeHandle
) -> HResult;

type ResumeProcessFn = unsafe extern "C" fn(Handle) -> HResult;

type StartupCallback = extern "C" fn(*mut c_void, *mut c_void, HResult);

type RegisterForRuntimeStartupFn = unsafe extern "C" fn(
    DWord,            // dwProcessId
    StartupCallback,  // pfnCallback
    *mut c_void,      // parameter
    *mut *mut c_void, // ppUnregisterToken
) -> HResult;

static mut CALLBACK_FIRED: bool = false;
static mut CALLBACK_HR: HResult = 0;
static mut P_CORDB: *mut c_void = ptr::null_mut();

extern "C" fn startup_callback(p_cordb: *mut c_void, _parameter: *mut c_void, hr: HResult) {
    eprintln!(
        "[callback] RegisterForRuntimeStartup disparou! pCordb={:?} hr=0x{:08x}",
        p_cordb, hr as u32
    );
    unsafe {
        CALLBACK_FIRED = true;
        CALLBACK_HR = hr;
        P_CORDB = p_cordb;
    }
}

fn to_utf16(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("uso: icordebug-spike <libdbgshim.so> <caminho-do-dll> [cwd]");
        std::process::exit(1);
    }
    let lib_path = &args[1];
    let dll_path = &args[2];
    let cwd = args.get(3).cloned().unwrap_or_else(|| ".".to_string());

    eprintln!("[spike] carregando {lib_path}...");
    let lib = unsafe { Library::new(lib_path) }.expect("falha ao carregar libdbgshim.so");

    let create_process_for_launch: libloading::Symbol<CreateProcessForLaunchFn> = unsafe {
        lib.get(b"CreateProcessForLaunch\0")
            .expect("símbolo CreateProcessForLaunch não encontrado")
    };
    let resume_process: libloading::Symbol<ResumeProcessFn> = unsafe {
        lib.get(b"ResumeProcess\0")
            .expect("símbolo ResumeProcess não encontrado")
    };
    let register_for_runtime_startup: libloading::Symbol<RegisterForRuntimeStartupFn> = unsafe {
        lib.get(b"RegisterForRuntimeStartup\0")
            .expect("símbolo RegisterForRuntimeStartup não encontrado")
    };

    let cmdline = format!("/usr/share/dotnet/dotnet {dll_path}");
    let mut cmdline_w = to_utf16(&cmdline);
    let cwd_w = to_utf16(&cwd);

    let mut pid: DWord = 0;
    let mut resume_handle: Handle = ptr::null_mut();

    eprintln!("[spike] CreateProcessForLaunch: {cmdline}");
    let t0 = Instant::now();
    let hr = unsafe {
        create_process_for_launch(
            cmdline_w.as_mut_ptr(),
            1, // suspenso, esperando o debugger
            ptr::null_mut(),
            cwd_w.as_ptr(),
            &mut pid,
            &mut resume_handle,
        )
    };
    eprintln!("[spike] CreateProcessForLaunch hr=0x{:08x} pid={pid}", hr as u32);
    if hr != 0 {
        std::process::exit(1);
    }

    let mut token: *mut c_void = ptr::null_mut();
    let hr2 = unsafe {
        register_for_runtime_startup(pid, startup_callback, ptr::null_mut(), &mut token)
    };
    eprintln!("[spike] RegisterForRuntimeStartup hr=0x{:08x}", hr2 as u32);

    eprintln!("[spike] ResumeProcess...");
    let hr3 = unsafe { resume_process(resume_handle) };
    eprintln!("[spike] ResumeProcess hr=0x{:08x}", hr3 as u32);

    eprintln!("[spike] esperando o callback de startup do runtime (até 10s)...");
    let deadline = Duration::from_secs(10);
    loop {
        if unsafe { CALLBACK_FIRED } {
            eprintln!(
                "[spike] SUCESSO: callback disparou após {:?}, hr=0x{:08x}",
                t0.elapsed(),
                unsafe { CALLBACK_HR as u32 }
            );
            break;
        }
        if t0.elapsed() > deadline {
            eprintln!("[spike] FALHA: callback nunca disparou em {:?}", deadline);
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let p_cordb = unsafe { P_CORDB };

    eprintln!("[spike] QueryInterface(IID_ICorDebug)...");
    let icordebug_ptr = match unsafe { com::query_interface(p_cordb, &IID_ICORDEBUG) } {
        Ok(p) => p,
        Err(hr) => {
            eprintln!("[spike] FALHA: QueryInterface deu hr=0x{:08x}", hr as u32);
            std::process::exit(1);
        }
    };
    let cordebug = CorDebug(icordebug_ptr);
    eprintln!("[spike] QueryInterface OK: {:?}", icordebug_ptr);

    eprintln!("[spike] ICorDebug::Initialize()...");
    let hr_init = unsafe { cordebug.initialize() };
    eprintln!("[spike] Initialize hr=0x{:08x}", hr_init as u32);
    if hr_init != S_OK {
        std::process::exit(1);
    }

    let mut callback_obj = ManagedCallbackObj {
        vtbl: &MANAGED_CALLBACK_VTBL,
        ref_count: 0,
    };
    let callback_ptr = &mut callback_obj as *mut ManagedCallbackObj as *mut c_void;

    eprintln!("[spike] ICorDebug::SetManagedHandler()...");
    let hr_handler = unsafe { cordebug.set_managed_handler(callback_ptr) };
    eprintln!("[spike] SetManagedHandler hr=0x{:08x}", hr_handler as u32);
    if hr_handler != S_OK {
        std::process::exit(1);
    }

    eprintln!("[spike] ICorDebug::DebugActiveProcess(pid={pid})...");
    match unsafe { cordebug.debug_active_process(pid) } {
        Ok(p_process) => eprintln!("[spike] SUCESSO: DebugActiveProcess retornou {:?}", p_process),
        Err(hr) => {
            eprintln!("[spike] FALHA: DebugActiveProcess deu hr=0x{:08x}", hr as u32);
            std::process::exit(1);
        }
    }

    eprintln!("[spike] anexado! esperando eventos do runtime (CreateProcess, LoadModule...) por 8s...");
    std::thread::sleep(Duration::from_secs(8));
    eprintln!("[spike] fim da espera.");
}
