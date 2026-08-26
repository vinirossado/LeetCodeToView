// The stepping/inspection half of the managed callback: arming the next
// step (plain vs. line-range), the two callbacks that drive it
// (Breakpoint — the very first stepper of the session — and StepComplete,
// re-armed on every single step), and everything StepComplete needs to
// inspect a frame (locals extraction, value-to-JSON conversion). Split out
// of `com/callback/mod.rs` purely because this cluster is large (~500
// lines) and self-contained — every private helper here is used only by
// the two callbacks in this same file.

use std::collections::BTreeMap;
use std::os::raw::c_void;

use super::super::icordebug::{
    continue_, create_stepper, get_active_frame, get_caller, get_function, get_function_module, get_function_token,
    get_il_offset, get_local_variable, get_metadata_import, step_into, step_range,
};
use super::super::values::{
    dereference, get_array_count, get_array_element_at, get_string_value, get_value_i32, get_value_type, is_null,
};
use super::super::{
    get_method_name, query_interface, HResult, IID_ICORDEBUG_ARRAY_VALUE, IID_ICORDEBUG_IL_FRAME,
    IID_ICORDEBUG_REFERENCE_VALUE, IID_ICORDEBUG_STRING_VALUE, S_OK,
};
use super::{
    LIMIT_SINK, LINE_RESOLVER, LOCAL_NAME_RESOLVER, SAMPLE_N, STEP_CAPPED, STEP_EVENTS_EMITTED, STEP_EVENTS_TOTAL,
    STEP_RANGE_RESOLVER, STEP_SINK, USER_MODULE,
};

/// Resolves the IL-offset range covering the CURRENT position of `thread`'s
/// active frame (see STEP_RANGE_RESOLVER's doc comment) — `None` whenever a
/// meaningful line range can't be computed (no active managed frame, no
/// PDB, hidden sequence point, or the frame isn't in the user's own module —
/// same module-gating rule LINE_RESOLVER/LOCAL_NAME_RESOLVER already use,
/// see USER_MODULE's doc comment for why: a step landed inside CoreCLR/BCL
/// internals must never get resolved against the user's own PDB just
/// because rids can coincidentally collide across modules).
unsafe fn resolve_step_range(thread: *mut c_void) -> Option<(u32, u32)> {
    let frame = get_active_frame(thread).ok()?;
    let il_frame = query_interface(frame, &IID_ICORDEBUG_IL_FRAME).ok()?;
    let offset = get_il_offset(il_frame).ok()?;
    let func = get_function(il_frame).ok()?;
    let method_token = get_function_token(func).ok()?;
    #[allow(static_mut_refs)]
    let is_user_module = !USER_MODULE.is_null() && get_function_module(func).ok() == Some(USER_MODULE);
    if !is_user_module {
        return None;
    }
    STEP_RANGE_RESOLVER.and_then(|resolve| resolve(method_token, offset))
}

/// Arms `stepper`'s next step: `StepRange` over `range` when one was
/// resolved (line-granular — see `resolve_step_range`/STEP_RANGE_RESOLVER),
/// else falls back to plain per-IL-instruction `Step` — the same fallback
/// used before line-range stepping existed, now scoped to just the cases
/// that genuinely can't be resolved to a real source line (see
/// `resolve_step_range`'s doc comment for the exact list).
unsafe fn arm_step(stepper: *mut c_void, range: Option<(u32, u32)>) -> HResult {
    match range {
        Some((start, end)) => step_range(stepper, true, start, end),
        None => step_into(stepper),
    }
}

pub(super) unsafe extern "C" fn cb_breakpoint(
    _this: *mut c_void,
    app_domain: *mut c_void,
    thread: *mut c_void,
    breakpoint: *mut c_void,
) -> HResult {
    eprintln!(
        "[callback] SUCESSO: Breakpoint disparou! thread={:?} breakpoint={:?}",
        thread, breakpoint
    );

    match create_stepper(thread) {
        Ok(stepper) => {
            eprintln!("[callback]   Stepper criado: {:?}", stepper);
            let range = resolve_step_range(thread);
            let hr_step = arm_step(stepper, range);
            eprintln!(
                "[callback]   arm_step(range={:?}) -> hr=0x{:08x}",
                range, hr_step as u32
            );
            let hr_cont = continue_(app_domain);
            eprintln!("[callback]   Continue() pra deixar o step acontecer -> hr=0x{:08x}", hr_cont as u32);
        }
        Err(hr) => {
            eprintln!("[callback]   FALHA ao criar stepper: hr=0x{:08x}", hr as u32);
        }
    }
    S_OK
}

// Must match events::STEP_EVENT_CAP in the lib crate — duplicated here
// (rather than `use crate::events::STEP_EVENT_CAP`) for the same reason
// STEP_SINK/LIMIT_SINK are indirections: this file is shared with the
// legacy icordebug-spike binary, which has no `events` module.
const STEP_EVENT_CAP: u32 = 5000;

/// Cap on how many call-stack frames also get a full `locals` snapshot in
/// the `frames` array (per-frame click-to-inspect in the call-stack panel,
/// tasks.md's Python-Tutor-inspired recursion-clarity item) — port of
/// jdi/Debugger.java's `MAX_FRAMES_WITH_LOCALS`, same value, same reasoning:
/// frame *names* (the `stack` array) stay cheap to walk even at deep
/// recursion (see get_caller's doc comment on the depth>50 guard below), but
/// extracting every frame's full `locals` (GetLocalVariable per slot, plus a
/// PDB `locals_for` lookup) is real per-frame cost that would reproduce the
/// exact quadratic-growth blowup MAX_STACK_FRAMES's Java-side sibling
/// constant already exists to avoid, just worse. 20 covers what a user could
/// plausibly click through in the call-stack panel anyway.
const MAX_FRAMES_WITH_LOCALS: usize = 20;

/// Cada StepComplete inspeciona o frame atual e (sujeito a amostragem, ver
/// abaixo) emite UM evento de step. Ao atingir o cap de 5.000 eventos
/// EMITIDOS (`STEP_EVENT_CAP`, mesma decisão de escopo do lado Java — ver
/// jdi/Debugger.java), para de armar um novo stepper e deixa o programa
/// terminar sozinho, sem overhead de instrumentação — emite
/// `step_limit_exceeded` uma única vez nesse momento. Se a inspeção falhar
/// (ex: sem frame gerenciado ativo, perto do fim da execução), não emite
/// nada nesse passo mas continua avançando de qualquer forma.
///
/// Sampling (port of jdi/Debugger.java's `spike.sample`/eventCount, see
/// SAMPLE_N/STEP_EVENTS_TOTAL in com/callback/mod.rs): STEP_EVENTS_TOTAL
/// counts every single StepComplete callback, cheaply, before any
/// inspection. The expensive extraction (get_active_frame/extract_locals/
/// get_call_stack_names) plus the STEP_SINK call only runs when
/// `STEP_EVENTS_TOTAL % SAMPLE_N == 0` — on the other N-1 out of N
/// callbacks this function does none of that work. This is purely about
/// which steps get INSPECTED; it never changes which steps get STEPPED —
/// create_stepper/step_into below still runs on every callback (until
/// STEP_CAPPED), so the debuggee's actual control flow and the
/// JDWP-equivalent ICorDebug round-trip cost are unaffected by SAMPLE_N,
/// exactly mirroring Java's semantics (see java.rs/Debugger.java comments
/// on eventCount vs emittedCount).
pub(super) unsafe extern "C" fn cb_step_complete(
    _this: *mut c_void,
    app_domain: *mut c_void,
    thread: *mut c_void,
    _stepper: *mut c_void,
    _reason: i32,
) -> HResult {
    // Resolved once per callback, unconditionally (not gated by sampling
    // like the expensive locals/call-stack extraction below is) — both the
    // sampled-inspection block AND the re-arm block at the bottom need the
    // current frame's IL offset/method token/module-membership, and these
    // particular COM getters (GetActiveFrame/GetIP/GetFunction/
    // GetFunctionToken/GetFunction's module) are cheap next to the per-step
    // round-trip cost this driver already pays on every single callback,
    // unlike full locals/call-stack extraction which stays sampling-gated.
    let frame_info = get_active_frame(thread).ok().and_then(|frame| {
        let il_frame = query_interface(frame, &IID_ICORDEBUG_IL_FRAME).ok()?;
        let offset = get_il_offset(il_frame).ok()?;
        let func = get_function(il_frame).ok();
        let method_token = func.and_then(|f| get_function_token(f).ok()).unwrap_or(0);
        // See USER_MODULE's doc comment: method rids are only unique WITHIN
        // a module, and steps land in framework (CoreLib/System.*) modules
        // constantly, not just the user's own — a rid that coincidentally
        // also exists in the user assembly's PDB must NOT be resolved
        // against it, or a framework-internal step can get a
        // plausible-but-wrong real line/name (or step RANGE — same rule now
        // applies to STEP_RANGE_RESOLVER below, not just LINE_RESOLVER).
        // `#[allow]`: a plain pointer equality read of a `static mut`, not a
        // reference — same non-issue as the other bare-value static reads
        // throughout this file.
        #[allow(static_mut_refs)]
        let is_user_module =
            !USER_MODULE.is_null() && func.and_then(|f| get_function_module(f).ok()) == Some(USER_MODULE);
        Some((il_frame, offset, method_token, is_user_module))
    });

    if !STEP_CAPPED {
        STEP_EVENTS_TOTAL += 1;
        if STEP_EVENTS_TOTAL % SAMPLE_N.max(1) == 0 {
            if let Some((il_frame, offset, method_token, is_user_module)) = frame_info {
                // Real source line, resolved from the Portable PDB's
                // SequencePoints blob (see pdb.rs) when possible — mirrors
                // the granularity Java's JDI already gives for free (a real
                // line number, not a bytecode offset). Falls back to the raw
                // IL offset (today's pre-existing behavior) when there's no
                // PDB, this method has no sequence point data, this exact
                // offset falls inside a "hidden" (compiler-generated, no
                // source mapping) region, or the frame isn't even in the
                // user's own module — see LINE_RESOLVER's and USER_MODULE's
                // doc comments above.
                let line = if is_user_module {
                    LINE_RESOLVER.and_then(|resolve| resolve(method_token, offset)).map(|l| l as i64)
                } else {
                    None
                }
                .unwrap_or(offset as i64);
                let locals = extract_locals(il_frame, method_token, offset, is_user_module);
                // walk_call_stack does ONE walk of GetCaller producing BOTH
                // `stack` (frame names, same as the old get_call_stack_names)
                // and `frames` (per-frame name+locals, capped at
                // MAX_FRAMES_WITH_LOCALS) — reuses the innermost frame's
                // already-computed `locals` above for frames[0] instead of
                // re-extracting it, same efficiency move as
                // jdi/Debugger.java's frameLocalsJson call for i==0.
                let (stack, frames) = walk_call_stack(il_frame, locals.clone());
                if let Some(sink) = STEP_SINK {
                    sink(line, locals, stack, frames);
                }
                STEP_EVENTS_EMITTED += 1;
                if STEP_EVENTS_EMITTED >= STEP_EVENT_CAP {
                    STEP_CAPPED = true;
                    if let Some(sink) = LIMIT_SINK {
                        sink();
                    }
                }
            }
        }
    }

    if !STEP_CAPPED {
        if let Ok(stepper) = create_stepper(thread) {
            // Arm the NEXT step over the CURRENT position's full source-line
            // IL range (see STEP_RANGE_RESOLVER/arm_step) instead of a raw
            // single IL instruction — this is what makes StepComplete fire
            // once per source LINE instead of once per IL instruction. Reuses
            // frame_info computed above instead of re-querying the frame
            // (resolve_step_range would do the exact same lookups again).
            let range = frame_info.and_then(|(_, offset, method_token, is_user_module)| {
                if !is_user_module {
                    return None;
                }
                STEP_RANGE_RESOLVER.and_then(|resolve| resolve(method_token, offset))
            });
            arm_step(stepper, range);
        }
    }
    continue_(app_domain);
    S_OK
}

/// Imprime uma variável: primitivo (i32 bruto) ou referência (string
/// dereferenciada via ICorDebugReferenceValue → ICorDebugStringValue).
const ELEMENT_TYPE_STRING: i32 = 0xE;
const ELEMENT_TYPE_ARRAY: i32 = 0x14;
const ELEMENT_TYPE_SZARRAY: i32 = 0x1D;

/// Cap on array elements serialized per local, same value as Java's
/// MAX_ARRAY_ELEMENTS (jdi/Debugger.java::serializeValue) — kept in sync
/// deliberately, so a large array doesn't produce a wildly different
/// payload size just because of which language ran it. C# doesn't need
/// Java's MAX_DEPTH/MAX_FIELDS/cycle-detection machinery yet: locals here
/// are only primitives/strings/flat arrays (no generic-object field
/// serialization — see tasks.md "Fase 2", explicitly out of scope), so
/// there's no recursion depth to cap, only the element count.
const MAX_ARRAY_ELEMENTS: u32 = 20;

/// Dereferencia um valor por referência (string, array, objeto): IsNull +
/// Dereference, retornando o ICorDebugValue de verdade por trás do ponteiro.
unsafe fn dereference_value(value: *mut c_void) -> Result<Option<*mut c_void>, HResult> {
    let ref_value = query_interface(value, &IID_ICORDEBUG_REFERENCE_VALUE)?;
    if is_null(ref_value)? {
        return Ok(None);
    }
    Ok(Some(dereference(ref_value)?))
}

/// Walks the call stack from `start_frame` (an already-QI'd
/// ICorDebugILFrame, e.g. from `query_interface(active_frame,
/// IID_ICORDEBUG_IL_FRAME)`), producing BOTH the frame-name list (`stack`,
/// same content/order/depth-cap `get_call_stack_names` used to compute
/// separately) AND, for the first `MAX_FRAMES_WITH_LOCALS` frames, each
/// frame's own `locals` (`frames`) — a single walk instead of two, since
/// `ICorDebugFrame::GetCaller` is itself a real ICorDebug round trip per
/// frame, same reasoning `cb_step_complete` already applies to
/// `frame_info`.
///
/// `frame0_locals` lets the caller pass in the innermost frame's
/// already-computed locals (from `extract_locals`, called separately for
/// the backward-compatible top-level `locals` field) instead of this
/// function re-extracting them — same "reuse frame 0" efficiency move as
/// `jdi/Debugger.java`'s `frameLocalsJson` call for `i==0`.
///
/// Every OTHER frame (every caller past the innermost) is suspended at its
/// own CALL SITE — the IL offset of the `call`/`callvirt` instruction that
/// invoked the callee, i.e. exactly what `ICorDebugILFrame::GetIP` already
/// returns for ANY frame, active or not. `pdb.rs::PortablePdb::locals_for`
/// already takes `(method_token, il_offset)` as independent parameters (not
/// hardcoded to the innermost frame), so resolving each caller's own local
/// scope is a direct extension of the exact lookup already done for the
/// active frame — verified by reading `locals_for`'s signature/body before
/// writing this, not assumed.
///
/// `ICorDebugFrame::GetCaller` is only documented to return an
/// `ICorDebugFrame`, not necessarily one that also implements
/// `ICorDebugILFrame` (e.g. a native/internal/thunk frame has no managed IL
/// view at all) — unlike name resolution (`GetFunction`/`GetCaller`, both
/// part of the base `ICorDebugFrame` interface, safe to call on any frame
/// kind), extracting locals needs `GetIP`/`GetLocalVariable`, both
/// `ICorDebugILFrame`-specific. So, unlike the old `get_call_stack_names`
/// (which never re-QI'd caller frames, safe there only because it never
/// called an IL-frame-specific method on one), every non-innermost frame
/// here goes through an explicit `query_interface(frame,
/// IID_ICORDEBUG_IL_FRAME)` before touching `GetIP`/`GetLocalVariable`. When
/// that QueryInterface fails for a given frame, that frame still gets a
/// `name` (name resolution never needed the IL-frame view) but its `locals`
/// comes back as an empty object — same honest-absence fallback as
/// `jdi/Debugger.java`'s `frameLocalsJson` catching
/// `AbsentInformationException` for a frame with no debug info.
unsafe fn walk_call_stack(
    start_frame: *mut c_void,
    frame0_locals: BTreeMap<String, serde_json::Value>,
) -> (Vec<String>, Vec<(String, BTreeMap<String, serde_json::Value>)>) {
    let mut frame = start_frame;
    let mut depth = 0usize;
    let mut names = Vec::new();
    let mut frames = Vec::new();
    let mut frame0_locals = Some(frame0_locals);
    loop {
        let func = get_function(frame).ok();
        let token = func.and_then(|f| get_function_token(f).ok());
        let name = (|| {
            let f = func?;
            let module = get_function_module(f).ok()?;
            let metadata = get_metadata_import(module).ok()?;
            let t = token?;
            get_method_name(metadata, t).ok()
        })();
        let name_str = match (&name, token) {
            (Some(n), _) => n.clone(),
            (None, Some(t)) => format!("token=0x{:08x}", t),
            (None, None) => "<GetFunction falhou>".to_string(),
        };
        names.push(name_str.clone());

        if depth < MAX_FRAMES_WITH_LOCALS {
            let locals = match frame0_locals.take() {
                Some(l) => l,
                None => (|| -> Option<BTreeMap<String, serde_json::Value>> {
                    let il_frame = query_interface(frame, &IID_ICORDEBUG_IL_FRAME).ok()?;
                    let offset = get_il_offset(il_frame).ok()?;
                    let method_token = token?;
                    // Same USER_MODULE gating rule as frame_info's own
                    // is_user_module above (see that comment for why) — a
                    // caller frame landed in a framework module must not
                    // get its locals resolved against the user's own PDB
                    // just because rids can coincidentally collide across
                    // modules.
                    #[allow(static_mut_refs)]
                    let is_user_module = !USER_MODULE.is_null()
                        && func.and_then(|f| get_function_module(f).ok()) == Some(USER_MODULE);
                    Some(extract_locals(il_frame, method_token, offset, is_user_module))
                })()
                .unwrap_or_default(),
            };
            frames.push((name_str, locals));
        }

        match get_caller(frame) {
            Ok(Some(caller)) => {
                frame = caller;
                depth += 1;
                if depth > 50 {
                    break; // proteção contra pilha corrompida/recursão absurda
                }
            }
            _ => break,
        }
    }
    (names, frames)
}

/// Enumera as variáveis locais do frame (índice 0, 1, 2, ... até
/// GetLocalVariable falhar — sinal de que passou do fim da lista de locals
/// da assinatura do método). Chave é o nome real da variável, resolvido via
/// LOCAL_NAME_RESOLVER (leitura do Portable PDB — ver pdb.rs) quando
/// `is_user_module` é true; cai de volta pra "local_N" (índice posicional
/// puro) quando não há resolver setado (icordebug-spike, o binário legado),
/// o resolver não achou nome pro slot (sem .pdb encontrado, ou índice fora
/// de qualquer LocalScope conhecido), OU `is_user_module` é false — ver
/// USER_MODULE's doc comment (com/callback/mod.rs) pra por que esse último
/// caso importa: sem ele, um frame de dentro do CoreCLR/BCL poderia por
/// coincidência reaproveitar um rid que também existe no PDB do usuário e
/// ganhar um nome de variável plausível mas errado, em vez de simplesmente
/// "local_N".
pub unsafe fn extract_locals(
    il_frame: *mut c_void,
    method_token: u32,
    il_offset: u32,
    is_user_module: bool,
) -> BTreeMap<String, serde_json::Value> {
    let names = if is_user_module {
        LOCAL_NAME_RESOLVER.map(|resolve| resolve(method_token, il_offset)).unwrap_or_default()
    } else {
        BTreeMap::new()
    };
    let mut locals = BTreeMap::new();
    for i in 0..64u32 {
        let value = match get_local_variable(il_frame, i) {
            Ok(v) => v,
            Err(_) => break,
        };
        let key = names.get(&i).cloned().unwrap_or_else(|| format!("local_{i}"));
        locals.insert(key, local_value_to_json(value));
    }
    locals
}

/// Converte um valor local pra JSON: primitivo (i32 bruto), string
/// dereferenciada, array dereferenciado (elementos lidos como i32), ou null
/// (referência nula ou qualquer falha na extração).
unsafe fn local_value_to_json(value: *mut c_void) -> serde_json::Value {
    let t = get_value_type(value).unwrap_or(-1);

    if t == ELEMENT_TYPE_STRING {
        return match dereference_value(value) {
            Ok(None) => serde_json::Value::Null,
            Ok(Some(dereferenced)) => match query_interface(dereferenced, &IID_ICORDEBUG_STRING_VALUE) {
                Ok(string_value) => match get_string_value(string_value) {
                    Ok(s) => serde_json::json!(s),
                    Err(_) => serde_json::Value::Null,
                },
                Err(_) => serde_json::Value::Null,
            },
            Err(_) => serde_json::Value::Null,
        };
    }

    if t == ELEMENT_TYPE_ARRAY || t == ELEMENT_TYPE_SZARRAY {
        return match dereference_value(value) {
            Ok(None) => serde_json::Value::Null,
            Ok(Some(dereferenced)) => match query_interface(dereferenced, &IID_ICORDEBUG_ARRAY_VALUE) {
                Ok(array_value) => match get_array_count(array_value) {
                    Ok(count) => {
                        // Cap at MAX_ARRAY_ELEMENTS (see doc comment above),
                        // same idea as Java's serializeValue: read only the
                        // capped elements, then append a truncation marker
                        // string (matching Java's "...(+N elementos)"
                        // pattern) instead of silently dropping the rest.
                        let cap = count.min(MAX_ARRAY_ELEMENTS);
                        let mut items = Vec::new();
                        for i in 0..cap {
                            let v = match get_array_element_at(array_value, i) {
                                Ok(elem) => get_value_i32(elem).unwrap_or(0),
                                Err(_) => 0,
                            };
                            items.push(serde_json::json!(v));
                        }
                        if count > cap {
                            items.push(serde_json::json!(format!(
                                "...(+{} elementos)",
                                count - cap
                            )));
                        }
                        serde_json::Value::Array(items)
                    }
                    Err(_) => serde_json::Value::Null,
                },
                Err(_) => serde_json::Value::Null,
            },
            Err(_) => serde_json::Value::Null,
        };
    }

    match get_value_i32(value) {
        Ok(v) => serde_json::json!(v),
        Err(_) => serde_json::Value::Null,
    }
}
