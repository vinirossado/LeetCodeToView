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
    continue_, create_stepper, get_active_frame, get_call_stack_names, get_function, get_function_module,
    get_function_token, get_il_offset, get_local_variable, step_into, step_range,
};
use super::super::values::{
    dereference, get_array_count, get_array_element_at, get_string_value, get_value_i32, get_value_type, is_null,
};
use super::super::{
    query_interface, HResult, IID_ICORDEBUG_ARRAY_VALUE, IID_ICORDEBUG_IL_FRAME, IID_ICORDEBUG_REFERENCE_VALUE,
    IID_ICORDEBUG_STRING_VALUE, S_OK,
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
                let stack = get_call_stack_names(il_frame);
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
                if let Some(sink) = STEP_SINK {
                    sink(line, locals, stack);
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
