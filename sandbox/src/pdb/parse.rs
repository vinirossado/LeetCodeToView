// Portable PDB metadata-table parser: row structs for the tables we care
// about (LocalScope/LocalVariable/MethodDebugInformation), plus the big
// `parse()` function that walks the `#Pdb`/`#Strings`/`#~`/`#Blob` streams
// and builds a `PortablePdb`. See `pdb/mod.rs`'s module doc comment for the
// overall format context.

use std::collections::BTreeMap;

use super::reader::{blob_slice, Reader};
use super::sequence_points::{parse_sequence_points_blob, SequencePointEntry};
use super::PortablePdb;

pub(super) struct StreamHeader {
    pub(super) offset: usize,
    #[allow(dead_code)]
    pub(super) size: usize,
}

/// Table numbers we care about, per the Portable PDB spec (0x30-0x37 range;
/// everything else is either a standard ECMA-335 type-system table, absent
/// from a standalone PDB file, or a PDB table we don't need).
const TABLE_METHOD_DEF: usize = 0x06;
const TABLE_DOCUMENT: usize = 0x30;
const TABLE_METHOD_DEBUG_INFO: usize = 0x31;
const TABLE_LOCAL_SCOPE: usize = 0x32;
const TABLE_LOCAL_VARIABLE: usize = 0x33;

pub(super) struct LocalScopeRow {
    pub(super) method_rid: u32,
    pub(super) variable_list: u32,
    pub(super) start_offset: u32,
    pub(super) length: u32,
}

pub(super) struct LocalVariableRow {
    pub(super) index: u16,
    pub(super) name: String,
}

/// One row of the MethodDebugInformation table (0x31). Unlike LocalScope,
/// this table has NO explicit "Method" column — per the Portable PDB spec it
/// is implicitly indexed 1:1 by MethodDef RID (row `i` describes the method
/// whose mdMethodDef rid is `i`), so the row's own position is its key.
struct MethodDebugInfoRow {
    /// Document table rid this method's code lives in, or 0 (nil) if the
    /// SequencePoints blob itself carries an initial document reference
    /// instead (see `parse_sequence_points_blob`'s doc comment). This
    /// project doesn't track *which* document a line came from (every
    /// snippet compiled by ProcessSandboxRunner is a single `Program.cs`, so
    /// there's only ever one document in practice) — only whether it's nil,
    /// which changes how many bytes the SequencePoints blob's header holds.
    document: u32,
    sequence_points_blob: u32,
}

pub(super) fn parse(data: &[u8]) -> Option<PortablePdb> {
    let r = Reader { data };

    if data.get(0..4)? != b"BSJB" {
        return None;
    }
    let version_len = r.u32(12)? as usize;
    let mut off = 16usize.checked_add(version_len)?;
    off = off.checked_add(2)?; // Flags (reserved)
    let stream_count = r.u16(off)?;
    off = off.checked_add(2)?;

    let mut streams: BTreeMap<&str, StreamHeader> = BTreeMap::new();
    for _ in 0..stream_count {
        let stream_offset = r.u32(off)? as usize;
        off = off.checked_add(4)?;
        let stream_size = r.u32(off)? as usize;
        off = off.checked_add(4)?;
        let name = r.c_str(off)?;
        let name_padded_len = (name.len() + 1).div_ceil(4) * 4;
        off = off.checked_add(name_padded_len)?;
        streams.insert(name, StreamHeader { offset: stream_offset, size: stream_size });
    }

    let pdb_stream = streams.get("#Pdb")?;
    let strings_stream = streams.get("#Strings")?;
    let tilde_stream = streams.get("#~")?;
    // #Blob is only needed for SequencePoints (line numbers) — unlike the
    // three streams above, its absence doesn't invalidate the whole parse
    // (local-name resolution doesn't touch it at all), so it's optional
    // here; sequence_points just ends up empty and line_for always returns
    // None, same "fall back, don't fail" spirit as everything else in this
    // file.
    let blob_stream = streams.get("#Blob");

    // #Pdb stream: 20-byte id, 4-byte entry point token, then a bitvector +
    // row-count array giving row counts for "type system" tables (like
    // MethodDef) that this standalone PDB references but doesn't itself
    // store — needed purely to size index columns correctly below.
    let mut p = pdb_stream.offset.checked_add(20)?.checked_add(4)?;
    let ref_tables_low = r.u32(p)? as u64;
    p = p.checked_add(4)?;
    let ref_tables_high = r.u32(p)? as u64;
    p = p.checked_add(4)?;
    let referenced_tables = ref_tables_low | (ref_tables_high << 32);

    let mut type_system_rows: BTreeMap<usize, u32> = BTreeMap::new();
    for bit in 0..64 {
        if referenced_tables & (1u64 << bit) != 0 {
            type_system_rows.insert(bit, r.u32(p)?);
            p = p.checked_add(4)?;
        }
    }

    // #~ (tables) stream header.
    let mut t = tilde_stream.offset.checked_add(4)?; // Reserved
    t = t.checked_add(2)?; // Major/MinorVersion
    let heap_sizes = *data.get(t)?;
    t = t.checked_add(2)?; // HeapSizes + Reserved
    let valid_low = r.u32(t)? as u64;
    t = t.checked_add(4)?;
    let valid_high = r.u32(t)? as u64;
    t = t.checked_add(4)?;
    let valid = valid_low | (valid_high << 32);
    t = t.checked_add(8)?; // Sorted bitvector

    let mut present_rows: BTreeMap<usize, u32> = BTreeMap::new();
    for table in 0..64usize {
        if valid & (1u64 << table) != 0 {
            present_rows.insert(table, r.u32(t)?);
            t = t.checked_add(4)?;
        }
    }

    let str_idx_wide = heap_sizes & 0x1 != 0;
    let guid_idx_wide = heap_sizes & 0x2 != 0;
    let blob_idx_wide = heap_sizes & 0x4 != 0;

    let row_count_of = |table: usize| -> u32 {
        present_rows.get(&table).copied().or_else(|| type_system_rows.get(&table).copied()).unwrap_or(0)
    };
    let idx_wide_for_table = |table: usize| row_count_of(table) > 0xFFFF;

    // Tables are laid out contiguously in increasing table-number order.
    // We only need to walk far enough to reach LocalVariable (0x33) —
    // whatever comes after (LocalConstant, ImportScope, ...) is irrelevant
    // here, so its row width is never computed.
    let document_idx_wide = idx_wide_for_table(TABLE_DOCUMENT);
    let method_def_idx_wide = idx_wide_for_table(TABLE_METHOD_DEF);
    let local_scope_self_idx_wide = idx_wide_for_table(0x35); // ImportScope
    let local_var_idx_wide = idx_wide_for_table(TABLE_LOCAL_VARIABLE);
    let local_const_idx_wide = idx_wide_for_table(0x34); // LocalConstant

    let width_of = |table: usize| -> Option<usize> {
        Some(match table {
            TABLE_DOCUMENT => {
                (if blob_idx_wide { 4 } else { 2 }) * 2 + (if guid_idx_wide { 4 } else { 2 }) * 2
            }
            TABLE_METHOD_DEBUG_INFO => {
                (if document_idx_wide { 4 } else { 2 }) + (if blob_idx_wide { 4 } else { 2 })
            }
            TABLE_LOCAL_SCOPE => {
                (if method_def_idx_wide { 4 } else { 2 })
                    + (if local_scope_self_idx_wide { 4 } else { 2 })
                    + (if local_var_idx_wide { 4 } else { 2 })
                    + (if local_const_idx_wide { 4 } else { 2 })
                    + 4
                    + 4
            }
            TABLE_LOCAL_VARIABLE => 2 + 2 + (if str_idx_wide { 4 } else { 2 }),
            _ => return None,
        })
    };

    let mut pos = t;
    let mut table_offset: BTreeMap<usize, usize> = BTreeMap::new();
    for (&table, &rows) in present_rows.iter() {
        if table > TABLE_LOCAL_VARIABLE {
            break;
        }
        table_offset.insert(table, pos);
        let width = width_of(table)?;
        pos = pos.checked_add(width.checked_mul(rows as usize)?)?;
    }

    let mut scopes = Vec::new();
    if let (Some(&base), Some(&rows)) = (table_offset.get(&TABLE_LOCAL_SCOPE), present_rows.get(&TABLE_LOCAL_SCOPE))
    {
        let width = width_of(TABLE_LOCAL_SCOPE)?;
        for i in 0..rows as usize {
            let mut o = base.checked_add(i.checked_mul(width)?)?;
            let method = r.idx(o, method_def_idx_wide)?;
            o = o.checked_add(if method_def_idx_wide { 4 } else { 2 })?;
            o = o.checked_add(if local_scope_self_idx_wide { 4 } else { 2 })?; // ImportScope, unused
            let variable_list = r.idx(o, local_var_idx_wide)?;
            o = o.checked_add(if local_var_idx_wide { 4 } else { 2 })?;
            o = o.checked_add(if local_const_idx_wide { 4 } else { 2 })?; // ConstantList, unused
            let start_offset = r.u32(o)?;
            o = o.checked_add(4)?;
            let length = r.u32(o)?;
            scopes.push(LocalScopeRow { method_rid: method, variable_list, start_offset, length });
        }
    }

    let mut variables = Vec::new();
    if let (Some(&base), Some(&rows)) =
        (table_offset.get(&TABLE_LOCAL_VARIABLE), present_rows.get(&TABLE_LOCAL_VARIABLE))
    {
        let width = width_of(TABLE_LOCAL_VARIABLE)?;
        for i in 0..rows as usize {
            let mut o = base.checked_add(i.checked_mul(width)?)?;
            let _attributes = r.u16(o)?;
            o = o.checked_add(2)?;
            let index = r.u16(o)?;
            o = o.checked_add(2)?;
            let name_idx = r.idx(o, str_idx_wide)? as usize;
            let name = r.c_str(strings_stream.offset.checked_add(name_idx)?)?.to_string();
            variables.push(LocalVariableRow { index, name });
        }
    }

    // MethodDebugInformation (0x31): NOT explicitly keyed by method — row i
    // (1-indexed) IS the debug info for MethodDef rid i, per the Portable
    // PDB spec. Read here purely to locate each method's SequencePoints
    // blob (Document is read too, only to know whether that blob's header
    // carries an extra InitialDocument field — see
    // parse_sequence_points_blob's doc comment).
    let mut method_debug_info = Vec::new();
    if let (Some(&base), Some(&rows)) =
        (table_offset.get(&TABLE_METHOD_DEBUG_INFO), present_rows.get(&TABLE_METHOD_DEBUG_INFO))
    {
        let width = width_of(TABLE_METHOD_DEBUG_INFO)?;
        for i in 0..rows as usize {
            let mut o = base.checked_add(i.checked_mul(width)?)?;
            let document = r.idx(o, document_idx_wide)?;
            o = o.checked_add(if document_idx_wide { 4 } else { 2 })?;
            let sequence_points_blob = r.idx(o, blob_idx_wide)?;
            method_debug_info.push(MethodDebugInfoRow { document, sequence_points_blob });
        }
    }

    let mut sequence_points: BTreeMap<u32, Vec<SequencePointEntry>> = BTreeMap::new();
    if let Some(blob_stream) = blob_stream {
        for (i, row) in method_debug_info.iter().enumerate() {
            // Nil blob (no debug info for this method at all, e.g. an
            // extern/compiler-synthesized method with no body) — nothing to
            // parse, and NOT the same case as a present-but-empty blob (idx
            // 0, the heap's always-empty entry): both end up with no
            // entries either way, so they're handled identically here, but
            // this early-continue avoids a wasted blob_slice call for the
            // (common) nil case.
            if row.sequence_points_blob == 0 {
                continue;
            }
            let Some(blob) = blob_slice(data, blob_stream.offset, row.sequence_points_blob) else { continue };
            let points = parse_sequence_points_blob(blob, row.document == 0);
            if !points.is_empty() {
                let rid = (i + 1) as u32; // MethodDebugInformation row i (0-indexed here) == MethodDef rid i+1
                sequence_points.insert(rid, points);
            }
        }
    }

    Some(PortablePdb { scopes, variables, sequence_points })
}
