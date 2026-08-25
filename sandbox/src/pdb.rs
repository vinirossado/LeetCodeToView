// Minimal Portable PDB reader — maps (method token, IL offset) to real local
// variable names AND real source line numbers. This is the piece that was
// previously investigated and deliberately deferred (see spec.md "Estratégia
// C#" / tasks.md): there's no native symbol-reader API in the .NET SDK
// (`ISymUnmanagedReader`) usable from COM interop, so this hand-rolled
// parser reads the Portable PDB metadata tables directly, per the format
// spec at https://github.com/dotnet/runtime/blob/main/docs/design/specs/PortablePDB-Metadata.md
// (itself an extension of the physical metadata layout in ECMA-335 §II.24).
//
// Two independent capabilities, both derived from the same loaded file:
//   - local variable names: Document/MethodDebugInformation (to find the
//     right byte offset) + LocalScope/LocalVariable (the actual data).
//   - source line numbers: the `SequencePoints` blob referenced by each
//     MethodDebugInformation row (via the #Blob heap) — a separate, more
//     complex, delta-compressed encoding. See the module comment right above
//     `parse_sequence_points_blob` below for the exact format (verified
//     against Microsoft's OWN reference implementation in
//     dotnet/runtime's System.Reflection.Metadata, not just the prose spec —
//     see that function's doc comment for how).
//
// Anything malformed or unexpected just yields `None`/an empty
// map/`Vec`/no-line-found — callers already fall back to positional `local_N`
// naming and the raw IL offset, same as before either capability existed, so
// there's no reason for a parse failure here to be fatal.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    fn u16(&self, offset: usize) -> Option<u16> {
        self.data.get(offset..offset + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&self, offset: usize) -> Option<u32> {
        self.data
            .get(offset..offset + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn idx(&self, offset: usize, wide: bool) -> Option<u32> {
        if wide {
            self.u32(offset)
        } else {
            self.u16(offset).map(|v| v as u32)
        }
    }

    /// Null-terminated string starting at `offset`, e.g. a stream name or a
    /// #Strings heap entry (both are UTF-8/ASCII, null-terminated).
    fn c_str(&self, offset: usize) -> Option<&'a str> {
        let bytes = self.data.get(offset..)?;
        let end = bytes.iter().position(|&b| b == 0)?;
        std::str::from_utf8(&bytes[..end]).ok()
    }
}

/// ECMA-335 §II.23.2 "compressed unsigned integer" — the variable-length
/// encoding used throughout metadata (heap indices, table row counts, AND,
/// relevant here, every field inside a SequencePoints blob). Verified
/// against the actual `MemoryBlock.PeekCompressedInteger` implementation in
/// dotnet/runtime's `System.Reflection.Metadata` (the library CoreCLR/Roslyn
/// themselves use to read this exact format), not just the prose spec:
///   - 1-byte form:  top bit 0,  value in the low 7 bits.
///   - 2-byte form:  top bits `10`, value in the low 14 bits, big-endian.
///   - 4-byte form:  top bits `110`, value in the low 29 bits, big-endian.
///
/// Returns `(value, bytes_consumed)`, or `None` on truncated/invalid input.
fn read_compressed_uint(data: &[u8], pos: usize) -> Option<(u32, usize)> {
    let b0 = *data.get(pos)?;
    if b0 & 0x80 == 0 {
        Some((b0 as u32, 1))
    } else if b0 & 0x40 == 0 {
        let b1 = *data.get(pos + 1)? as u32;
        Some((((b0 as u32 & 0x3f) << 8) | b1, 2))
    } else if b0 & 0x20 == 0 {
        let b1 = *data.get(pos + 1)? as u32;
        let b2 = *data.get(pos + 2)? as u32;
        let b3 = *data.get(pos + 3)? as u32;
        Some((((b0 as u32 & 0x1f) << 24) | (b1 << 16) | (b2 << 8) | b3, 4))
    } else {
        None
    }
}

/// ECMA-335 §II.23.2 "compressed signed integer" — used for the delta fields
/// in a SequencePoints blob (column/line deltas after the first record).
/// Verified against dotnet/runtime's actual
/// `BlobReader.TryReadCompressedSignedInteger`: read the SAME bit pattern as
/// the unsigned form above, then the low bit of the decoded magnitude is the
/// sign flag (1 = negative), and the true value is `magnitude >> 1`,
/// sign-extended from the number of DATA bits the encoding actually held (6
/// bits for the 1-byte form, 13 for 2-byte, 28 for 4-byte) when that flag is
/// set. This is NOT plain two's-complement of the compressed bytes — the
/// sign bit is folded into bit 0 of the pre-shift value, not the top bit.
fn read_compressed_int(data: &[u8], pos: usize) -> Option<(i32, usize)> {
    let (raw, len) = read_compressed_uint(data, pos)?;
    let sign_extend = raw & 1 != 0;
    let mut value = (raw >> 1) as i32;
    if sign_extend {
        value |= match len {
            1 => 0xffffffc0u32 as i32,
            2 => 0xffffe000u32 as i32,
            _ => 0xf0000000u32 as i32, // len == 4
        };
    }
    Some((value, len))
}

/// A #Blob heap entry is itself length-prefixed: a compressed unsigned
/// integer giving the byte length, immediately followed by that many raw
/// bytes (ECMA-335 §II.24.2.4). `idx` 0 is the heap's always-present empty
/// blob (a single 0x00 length byte) — no special-casing needed, the generic
/// read already yields an empty slice for it.
fn blob_slice(data: &[u8], heap_base: usize, idx: u32) -> Option<&[u8]> {
    let start = heap_base.checked_add(idx as usize)?;
    let (len, n) = read_compressed_uint(data, start)?;
    let content_start = start.checked_add(n)?;
    let content_end = content_start.checked_add(len as usize)?;
    data.get(content_start..content_end)
}

struct StreamHeader {
    offset: usize,
    #[allow(dead_code)]
    size: usize,
}

/// Table numbers we care about, per the Portable PDB spec (0x30-0x37 range;
/// everything else is either a standard ECMA-335 type-system table, absent
/// from a standalone PDB file, or a PDB table we don't need).
const TABLE_METHOD_DEF: usize = 0x06;
const TABLE_DOCUMENT: usize = 0x30;
const TABLE_METHOD_DEBUG_INFO: usize = 0x31;
const TABLE_LOCAL_SCOPE: usize = 0x32;
const TABLE_LOCAL_VARIABLE: usize = 0x33;

struct LocalScopeRow {
    method_rid: u32,
    variable_list: u32,
    start_offset: u32,
    length: u32,
}

struct LocalVariableRow {
    index: u16,
    name: String,
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

/// One resolved sequence point: the IL offset it starts at, and the source
/// line it maps to — or `None` if it's a "hidden" sequence point (compiler-
/// generated code with deliberately no source mapping, e.g. parts of a
/// `for` loop's condition/increment, or async state-machine plumbing; see
/// `parse_sequence_points_blob`). A `None` here is NOT a parse failure, it's
/// the format's own explicit way of saying "no real line applies to this IL
/// range" — `PortablePdb::line_for` deliberately does not substitute the
/// nearest real line for these, same "don't invent data" spirit as the rest
/// of this file's fallback philosophy (see `locals_for`'s handling of
/// unknown scopes).
struct SequencePointEntry {
    il_offset: u32,
    line: Option<u32>,
}

pub struct PortablePdb {
    scopes: Vec<LocalScopeRow>,
    variables: Vec<LocalVariableRow>,
    /// Method rid -> its sequence points, sorted ascending by `il_offset`
    /// (guaranteed by the blob encoding itself: IL-offset deltas after the
    /// first record are "unsigned compressed, non-zero", i.e. always
    /// forward). Only methods with a non-empty SequencePoints blob get an
    /// entry; `line_for` treats a missing rid the same as an empty `Vec` —
    /// no data, fall back to the raw IL offset.
    sequence_points: BTreeMap<u32, Vec<SequencePointEntry>>,
}

impl PortablePdb {
    /// Loads and parses the `.pdb` sibling of `dll_file` (same stem, next to
    /// it — how `dotnet build -c Debug` lays it out by default, see
    /// ProcessSandboxRunner.compileCsharp on the API side). Returns `None`
    /// on any I/O or format problem.
    pub fn load(dll_file: &Path) -> Option<PortablePdb> {
        let pdb_path = dll_file.with_extension("pdb");
        let data = fs::read(pdb_path).ok()?;
        parse(&data)
    }

    /// Real variable names visible at `il_offset` inside the method
    /// identified by `method_token` (an mdMethodDef, i.e. 0x06000000 | rid).
    /// Falls back to the union of every scope in the method if the offset
    /// doesn't land cleanly inside any single scope (e.g. right on a
    /// boundary) — still better than nothing.
    pub fn locals_for(&self, method_token: u32, il_offset: u32) -> BTreeMap<u32, String> {
        let rid = method_token & 0x00FF_FFFF;
        let mut in_scope = BTreeMap::new();
        let mut any_scope = BTreeMap::new();

        for (i, scope) in self.scopes.iter().enumerate() {
            if scope.method_rid != rid {
                continue;
            }
            let start = scope.variable_list.saturating_sub(1) as usize;
            let end = self
                .scopes
                .get(i + 1)
                .map(|next| next.variable_list.saturating_sub(1) as usize)
                .unwrap_or(self.variables.len())
                .min(self.variables.len());
            let Some(vars) = self.variables.get(start.min(end)..end) else {
                continue;
            };
            let in_range = il_offset >= scope.start_offset
                && il_offset < scope.start_offset.saturating_add(scope.length);
            for var in vars {
                any_scope.insert(var.index as u32, var.name.clone());
                if in_range {
                    in_scope.insert(var.index as u32, var.name.clone());
                }
            }
        }

        if in_scope.is_empty() {
            any_scope
        } else {
            in_scope
        }
    }

    /// Real source line at `il_offset` inside `method_token`, if resolvable.
    /// A sequence point covers every IL offset from its own `il_offset` up
    /// to (but not including) the next sequence point's `il_offset` in the
    /// same method — so this finds the LAST entry whose `il_offset` is `<=`
    /// the query and returns its line (`None` if that covering point is
    /// itself hidden, or if the method has no sequence point data at all,
    /// e.g. not found in this PDB, or `il_offset` is before the method's
    /// first sequence point). Callers (see `com.rs::cb_step_complete`) fall
    /// back to the raw IL offset in every `None` case, same fallback
    /// philosophy as `locals_for`'s `local_N` positional names.
    pub fn line_for(&self, method_token: u32, il_offset: u32) -> Option<u32> {
        let rid = method_token & 0x00FF_FFFF;
        let points = self.sequence_points.get(&rid)?;
        let idx = points.partition_point(|p| p.il_offset <= il_offset);
        if idx == 0 {
            return None;
        }
        points[idx - 1].line
    }
}

/// Parses the `SequencePoints` blob of a single `MethodDebugInformation` row
/// — the IL-offset -> source-line mapping. This is a materially different,
/// more complex encoding than `LocalScope`/`LocalVariable`'s fixed-width
/// table rows: a stream of delta-compressed, variable-length records, one
/// per sequence point, read left-to-right with running state (previous IL
/// offset, previous non-hidden start line/column) carried between records.
///
/// Format verified against the ACTUAL reference implementation, not just
/// the prose spec (https://github.com/dotnet/runtime/blob/main/docs/design/specs/PortablePDB-Metadata.md
/// "Sequence Points Blob"): Microsoft's own
/// `System.Reflection.Metadata.SequencePointCollection.Enumerator.MoveNext`
/// (dotnet/runtime, same file CoreCLR/Roslyn/ilasm/ildasm themselves use to
/// read this format) — fetched and read directly from GitHub before writing
/// this function, per this project's established "don't guess the encoding"
/// rule for PDB parsing. This function is a line-for-line port of that
/// method's control flow, minus the parts we don't need (Document/EndLine/
/// EndColumn tracking — see field docs on `SequencePointEntry`/
/// `MethodDebugInfoRow` for why those are safe to drop for this project).
///
/// Blob layout:
///   header:      LocalSignature (uint, unused/skipped)
///                [InitialDocument (uint)]  <- ONLY if the row's own
///                                             `Document` column is nil
///   record 1:    ILOffset (uint, absolute)
///                DeltaLines (uint)
///                DeltaColumns (uint if DeltaLines==0, else int)
///                [-- if DeltaLines==0 AND DeltaColumns==0: HIDDEN, stop here --]
///                StartLine (uint)      <- absolute, first non-hidden record
///                StartColumn (uint)
///   record N>1:  DeltaILOffset (uint, non-zero) -- OR, if exactly 0, this
///                is a "document record" instead: Document (uint) follows,
///                then the NEXT uint is really this record's DeltaILOffset
///                (a method can reference more than one source document;
///                every document-record just changes "current document" and
///                is otherwise transparent to IL-offset/line tracking, which
///                is all this project needs)
///                DeltaLines, DeltaColumns (same rules as above)
///                [-- hidden check, same as above --]
///                DeltaStartLine (int, relative to previous NON-HIDDEN start line)
///                DeltaStartColumn (int, relative to previous NON-HIDDEN start column)
///
/// KNOWN LIMITATION (honest, not silently swallowed): a method whose body
/// spans multiple source documents (e.g. a `partial` class split across
/// files) would have every one of its lines attributed correctly regardless
/// of which document they're in — line numbers within a document-record are
/// unaffected by which document is "current" — but if two documents in the
/// same method happened to reuse the same line NUMBER, this parser can't
/// tell them apart (it doesn't track Document at all). Not a concern for
/// this project today: ProcessSandboxRunner always compiles a single
/// `Program.cs`, so no PDB it ever produces can have more than one document.
fn parse_sequence_points_blob(blob: &[u8], document_is_nil: bool) -> Vec<SequencePointEntry> {
    let mut out = Vec::new();
    let mut pos = 0usize;

    // Header: LocalSignature rid (unused — that's the StandAloneSig for
    // hoisted locals in iterators/async state machines, not needed for line
    // resolution) and, only when the row's Document column is nil, the
    // method's initial document rid (also unused, see doc comment above).
    let Some((_local_sig, n)) = read_compressed_uint(blob, pos) else { return out };
    pos += n;
    if document_is_nil {
        let Some((_doc, n)) = read_compressed_uint(blob, pos) else { return out };
        pos += n;
    }

    let mut current_offset: u32 = 0;
    let mut have_current = false;
    // -1 sentinel = "no non-hidden start line seen yet" (mirrors the
    // reference implementation's `_previousNonHiddenStartLine = -1`); a real
    // line is always >= 0, so this can't collide with a real value.
    let mut prev_start_line: i64 = -1;

    loop {
        if pos >= blob.len() {
            break;
        }

        let offset = if !have_current {
            // First record: IL offset is absolute, not a delta.
            let Some((off, n)) = read_compressed_uint(blob, pos) else { break };
            pos += n;
            off
        } else {
            // Skip zero-delta "document record"s (this method switched
            // source document) until a real non-zero IL-offset delta shows
            // up — see the KNOWN LIMITATION note above for why the document
            // rid itself is read-and-discarded rather than tracked.
            loop {
                let Some((delta, n)) = read_compressed_uint(blob, pos) else { return out };
                pos += n;
                if delta == 0 {
                    let Some((_doc, n2)) = read_compressed_uint(blob, pos) else { return out };
                    pos += n2;
                    continue;
                }
                break current_offset.wrapping_add(delta);
            }
        };
        current_offset = offset;
        have_current = true;

        let Some((delta_lines, n)) = read_compressed_uint(blob, pos) else { break };
        pos += n;

        // Per the format, DeltaColumns is unsigned when DeltaLines==0
        // (same-line move, must be non-zero to be meaningful — except for
        // the hidden-point special case below) and signed otherwise (can
        // move the column left on a new line).
        let delta_columns: i32 = if delta_lines == 0 {
            let Some((dc, n)) = read_compressed_uint(blob, pos) else { break };
            pos += n;
            dc as i32
        } else {
            let Some((dc, n)) = read_compressed_int(blob, pos) else { break };
            pos += n;
            dc
        };

        // Hidden sequence point: signaled by DeltaLines == DeltaColumns == 0
        // (see the spec's "Start Line = End Line = 0xfeefee..." framing —
        // System.Reflection.Metadata's own reader checks it exactly this
        // way, via the zero deltas, not by reading a literal 0xfeefee).
        // No StartLine/StartColumn follow for a hidden point, and it does
        // NOT update `prev_start_line` (matches the reference: hidden points
        // are invisible to the "previous non-hidden" delta chain).
        if delta_lines == 0 && delta_columns == 0 {
            out.push(SequencePointEntry { il_offset: current_offset, line: None });
            continue;
        }

        let start_line = if prev_start_line < 0 {
            let Some((line, n)) = read_compressed_uint(blob, pos) else { break };
            pos += n;
            let Some((_col, n2)) = read_compressed_uint(blob, pos) else { break }; // start column, unused
            pos += n2;
            line as i64
        } else {
            let Some((dline, n)) = read_compressed_int(blob, pos) else { break };
            pos += n;
            let Some((_dcol, n2)) = read_compressed_int(blob, pos) else { break }; // column delta, unused
            pos += n2;
            prev_start_line + dline as i64
        };
        prev_start_line = start_line;
        out.push(SequencePointEntry { il_offset: current_offset, line: Some(start_line.max(0) as u32) });
    }

    out
}

fn parse(data: &[u8]) -> Option<PortablePdb> {
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

#[cfg(test)]
mod tests {
    use super::*;

    // Real Portable PDB emitted by `dotnet build -c Debug` (net8.0 SDK) for
    // a top-level-statements program, checked in as bytes rather than
    // regenerated at test time — this crate's CI doesn't have the .NET SDK
    // installed (see ci-sandbox.yml), so the test has to work from a fixed
    // fixture. Source that produced it (for reference, not compiled here):
    //
    //   int counter = 0;
    //   string message = "hello";
    //   int[] items = { 1, 2, 3 };
    //   for (int i = 0; i < items.Length; i++) { counter += items[i]; }
    //   if (counter > 0) { string extra = "positive"; Console.WriteLine(extra); }
    //   Console.WriteLine(message + counter);
    const FIXTURE: &[u8] = include_bytes!("testdata/toplevel_statements.pdb");

    #[test]
    fn resolves_names_for_every_scope_in_the_top_level_entry_point() {
        let pdb = parse(FIXTURE).expect("fixture should parse as a valid Portable PDB");
        assert!(!pdb.scopes.is_empty());
        assert!(!pdb.variables.is_empty());

        // Union every name visible anywhere in the (single, in this
        // fixture) user method, regardless of exact RID — the compiler's
        // internal token numbering for top-level statements isn't a stable
        // thing to hardcode a test against.
        let mut all_names = std::collections::BTreeSet::new();
        for rid in 1..=8u32 {
            for name in pdb.locals_for(0x0600_0000 | rid, 0).values() {
                all_names.insert(name.clone());
            }
            // Union across the whole method body too (not just offset 0),
            // by probing a few offsets spread across the method.
            for offset in [10, 30, 70, 200] {
                for name in pdb.locals_for(0x0600_0000 | rid, offset).values() {
                    all_names.insert(name.clone());
                }
            }
        }

        for expected in ["counter", "message", "items", "i", "extra"] {
            assert!(all_names.contains(expected), "expected {expected:?} in {all_names:?}");
        }
    }

    #[test]
    fn scoped_lookup_excludes_out_of_scope_locals() {
        let pdb = parse(FIXTURE).expect("fixture should parse");
        // Find whichever method actually has scopes (the entry point).
        let rid = pdb.scopes.first().map(|s| s.method_rid).expect("fixture has at least one scope");
        let token = 0x0600_0000 | rid;

        // At IL offset 0 (function entry), the `for` loop's `i` and the
        // `if` block's `extra` shouldn't be visible yet — only the
        // top-level locals declared before any nested block.
        let at_entry = pdb.locals_for(token, 0);
        let names: std::collections::BTreeSet<_> = at_entry.values().cloned().collect();
        assert!(names.contains("counter"));
        assert!(!names.contains("extra"), "extra is scoped to the if-block, shouldn't be visible at offset 0");
    }

    #[test]
    fn missing_pdb_file_yields_none() {
        assert!(PortablePdb::load(Path::new("/nonexistent/path/app.dll")).is_none());
    }

    // Real Portable PDB emitted by `dotnet build -c Debug` (net8.0 SDK),
    // deliberately NOT straight-line code — a `for` loop with an `if`/`else`
    // branch inside its body, calling two different one-line static
    // methods, per this task's own instruction to avoid a fixture where a
    // naive/wrong offset->line mapping could look correct by coincidence.
    // Source that produced it (for reference, not compiled here — line
    // numbers below are 1-indexed against exactly this text):
    //
    //  1: int total = 0;
    //  2: for (int i = 0; i < 5; i++)
    //  3: {
    //  4:     if (i % 2 == 0)
    //  5:     {
    //  6:         total += Helper.DoubleIt(i);
    //  7:     }
    //  8:     else
    //  9:     {
    // 10:         total += Helper.TripleIt(i);
    // 11:     }
    // 12: }
    // 13: Console.WriteLine(total);
    // 14: (blank)
    // 15: static class Helper
    // 16: {
    // 17:     public static int DoubleIt(int x)
    // 18:     {
    // 19:         int result = x * 2;
    // 20:         return result;
    // 21:     }
    // 22: (blank)
    // 23:     public static int TripleIt(int x)
    // 24:     {
    // 25:         int result = x * 3;
    // 26:         return result;
    // 27:     }
    // 28: }
    //
    // Ground truth for every (offset, line-or-hidden) pair below was NOT
    // hand-derived from this source — it was dumped directly from this same
    // .pdb file using Microsoft's OWN official Portable PDB reader
    // (System.Reflection.Metadata's `SequencePointCollection`, via a
    // throwaway `dotnet run` program), so this test checks this parser
    // against the reference implementation's real output, not against a
    // possibly-wrong manual reading of the spec.
    const BRANCHING_LOOP_FIXTURE: &[u8] = include_bytes!("testdata/branching_loop.pdb");

    #[test]
    fn resolves_real_lines_for_top_level_entry_point_with_loop_and_branch() {
        let pdb = parse(BRANCHING_LOOP_FIXTURE).expect("fixture should parse");
        let token = 0x0600_0001; // <Main>$, confirmed by the oracle dump

        // Exact (offset, expected line) pairs, straight from the oracle.
        // Includes the compiler-generated "hidden" points the for-loop's
        // condition-check/branch code produces (offsets 4, 14, 28, 51) —
        // real, not synthetic: a for-loop is exactly the kind of construct
        // the task asked for specifically because straight-line code
        // wouldn't exercise hidden points at all.
        let exact: &[(u32, Option<u32>)] = &[
            (0, Some(1)),
            (2, Some(2)),
            (4, None), // hidden
            (6, Some(3)),
            (7, Some(4)),
            (14, None), // hidden
            (17, Some(5)),
            (18, Some(6)), // total += Helper.DoubleIt(i) — the loop body
            (27, Some(7)),
            (28, None), // hidden
            (30, Some(9)),
            (31, Some(10)), // total += Helper.TripleIt(i) — the other branch
            (40, Some(11)),
            (41, Some(12)),
            (42, Some(2)), // back-edge: loop increment maps back to line 2
            (46, Some(2)), // loop condition re-check, still line 2
            (51, None),    // hidden
            (54, Some(13)),
        ];
        for &(offset, expected) in exact {
            assert_eq!(
                pdb.line_for(token, offset),
                expected,
                "offset {offset}: expected {expected:?}"
            );
        }

        // Coverage semantics (not just the exact recorded offsets): every IL
        // offset between two consecutive sequence points resolves to the
        // EARLIER point's line, proving this isn't just an exact-match
        // lookup table. Offset 3 sits between the (2, line 2) and
        // (4, hidden) records.
        assert_eq!(pdb.line_for(token, 3), Some(2));
        // Offset 13 sits inside the (7, line 4)..(14, hidden) range.
        assert_eq!(pdb.line_for(token, 13), Some(4));
        // Offset 16 sits inside the (14, hidden)..(17, line 5) range — still
        // hidden, must NOT leak the next real line early.
        assert_eq!(pdb.line_for(token, 16), None);
        // Offset 53 sits inside the (51, hidden)..(54, line 13) range.
        assert_eq!(pdb.line_for(token, 53), None);
        // Offset 1000 is past the method's last sequence point — still
        // resolves to that last point's line (IL offsets keep going up to
        // `ret`, sequence points don't cover every single instruction).
        assert_eq!(pdb.line_for(token, 1000), Some(13));
    }

    #[test]
    fn resolves_real_lines_for_helper_methods_confirming_per_method_token_lookup() {
        let pdb = parse(BRANCHING_LOOP_FIXTURE).expect("fixture should parse");

        // Helper.DoubleIt — confirmed token via the oracle dump.
        let double_it = 0x0600_0003;
        assert_eq!(pdb.line_for(double_it, 0), Some(18));
        assert_eq!(pdb.line_for(double_it, 1), Some(19)); // int result = x * 2;
        assert_eq!(pdb.line_for(double_it, 5), Some(20)); // return result;
        assert_eq!(pdb.line_for(double_it, 9), Some(21));

        // Helper.TripleIt — a DIFFERENT method token with its OWN sequence
        // points, proving line_for keys off method_token, not just offset.
        let triple_it = 0x0600_0004;
        assert_eq!(pdb.line_for(triple_it, 0), Some(24));
        assert_eq!(pdb.line_for(triple_it, 1), Some(25)); // int result = x * 3;
        assert_eq!(pdb.line_for(triple_it, 5), Some(26));
        assert_eq!(pdb.line_for(triple_it, 9), Some(27));

        // Same IL offset (1), different method tokens, different real
        // lines — the strongest evidence this isn't accidentally reading a
        // shared/global offset table.
        assert_ne!(pdb.line_for(double_it, 1), pdb.line_for(triple_it, 1));
    }

    #[test]
    fn line_for_falls_back_to_none_for_unknown_method_or_missing_data() {
        let pdb = parse(BRANCHING_LOOP_FIXTURE).expect("fixture should parse");
        // A method token that doesn't exist in this assembly at all.
        assert_eq!(pdb.line_for(0x0600_0099, 0), None);
        // The older fixture (checked in before this task, for local-name
        // resolution only) still parses fine under the new, additive
        // sequence-points logic — and, since it's also a real net8.0
        // Portable PDB, resolving lines from it works too (cross-checked
        // against the same oracle dump technique: offset 0 of the entry
        // point is real source line 1, `int counter = 0;`).
        let older = parse(FIXTURE).expect("older fixture should still parse");
        assert_eq!(older.line_for(0x0600_0001, 0), Some(1));
    }
}
