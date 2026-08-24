// Minimal Portable PDB reader — just enough to map (method token, IL offset)
// to real local variable names. This is the piece that was previously
// investigated and deliberately deferred (see spec.md "Estratégia C#" /
// tasks.md): there's no native symbol-reader API in the .NET SDK
// (`ISymUnmanagedReader`) usable from COM interop, so this hand-rolled
// parser reads the Portable PDB metadata tables directly, per the format
// spec at https://github.com/dotnet/runtime/blob/main/docs/design/specs/PortablePDB-Metadata.md
// (itself an extension of the physical metadata layout in ECMA-335 §II.24).
//
// Deliberately narrow: only the four tables needed to resolve local variable
// names are parsed (Document/MethodDebugInformation to correctly skip to the
// right byte offset, LocalScope/LocalVariable for the actual data). Anything
// malformed or unexpected just yields `None`/an empty map — callers already
// fall back to positional `local_N` naming, same as before this existed, so
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

pub struct PortablePdb {
    scopes: Vec<LocalScopeRow>,
    variables: Vec<LocalVariableRow>,
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

    Some(PortablePdb { scopes, variables })
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
}
