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
//     complex, delta-compressed encoding. See the module comment on
//     `sequence_points::parse_sequence_points_blob` for the exact format
//     (verified against Microsoft's OWN reference implementation in
//     dotnet/runtime's System.Reflection.Metadata, not just the prose spec —
//     see that function's doc comment for how).
//
// Anything malformed or unexpected just yields `None`/an empty
// map/`Vec`/no-line-found — callers already fall back to positional `local_N`
// naming and the raw IL offset, same as before either capability existed, so
// there's no reason for a parse failure here to be fatal.
//
// Module layout: `reader` holds the low-level binary-reading primitives (no
// PDB-specific semantics), `parse` holds the metadata-table row structs and
// the big `parse()` function, `sequence_points` holds the SequencePoints
// blob's own (more complex) delta-decoding, and this file holds the public
// `PortablePdb` API surface the rest of the crate calls into.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

mod parse;
mod reader;
mod sequence_points;

use parse::{LocalScopeRow, LocalVariableRow};
use sequence_points::SequencePointEntry;

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
        parse::parse(&data)
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
    /// first sequence point). Callers (see `com/callback.rs::cb_step_complete`)
    /// fall back to the raw IL offset in every `None` case, same fallback
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

    /// The half-open IL-offset range `[start, end)` of the sequence point
    /// covering `il_offset` inside `method_token` — i.e. exactly the IL
    /// extent of the CURRENT source line, the input
    /// `ICorDebugStepper::StepRange` needs to step by source line instead of
    /// by raw IL instruction (see com/callback.rs's STEP_RANGE_RESOLVER/arm_step).
    /// `start` is the covering point's own `il_offset` (same one `line_for`
    /// would key off of); `end` is the NEXT sequence point's `il_offset` in
    /// the same method, or `u32::MAX` if this is the method's last recorded
    /// sequence point — safe as an upper bound because `StepRange` is scoped
    /// to the stepper's own frame regardless of how large `end` is: leaving
    /// the frame (return, exception unwind, ...) always completes the step
    /// on its own, per `StepRange`'s own doc comment in cordebug.idl ("will
    /// not complete until code outside the given range is reached" — a
    /// popped frame is trivially outside any IL range of that frame's own
    /// method). We don't otherwise track a method's total IL body length, so
    /// this sentinel avoids needing to.
    ///
    /// Returns `None` in every case `line_for` would (no PDB data for this
    /// method — not found in this PDB, or a totally different rid coming
    /// from another module entirely, see USER_MODULE's doc comment in
    /// com/callback.rs — or `il_offset` before the method's first sequence
    /// point), PLUS one `line_for` doesn't need to special-case: when the
    /// covering sequence point is "hidden" (`line: None`, compiler-generated
    /// code with no real source line — see `SequencePointEntry`'s doc
    /// comment). There's no meaningful *source line* range to step by there,
    /// so the caller (com/callback.rs's `arm_step`) honestly falls back to
    /// plain single-instruction `Step` for just that one hop, same "don't
    /// invent data" fallback philosophy as `line_for`/`locals_for`.
    pub fn step_range_for(&self, method_token: u32, il_offset: u32) -> Option<(u32, u32)> {
        let rid = method_token & 0x00FF_FFFF;
        let points = self.sequence_points.get(&rid)?;
        let idx = points.partition_point(|p| p.il_offset <= il_offset);
        if idx == 0 {
            return None;
        }
        let current = &points[idx - 1];
        current.line?;
        let end = points.get(idx).map(|p| p.il_offset).unwrap_or(u32::MAX);
        Some((current.il_offset, end))
    }
}

#[cfg(test)]
mod tests {
    use super::parse::parse;
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
    const FIXTURE: &[u8] = include_bytes!("../testdata/toplevel_statements.pdb");

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
    const BRANCHING_LOOP_FIXTURE: &[u8] = include_bytes!("../testdata/branching_loop.pdb");

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
    fn step_range_for_covers_exactly_one_source_line_per_range() {
        let pdb = parse(BRANCHING_LOOP_FIXTURE).expect("fixture should parse");
        let token = 0x0600_0001; // <Main>$, same fixture/token as the line_for test above

        // Same recorded IL offsets as the line_for test's "exact" table
        // above, but here checking the IL RANGE each one's sequence point
        // covers — (start, end) is (this offset, next recorded offset), or
        // (this offset, u32::MAX) for the method's very last sequence
        // point. Hidden points (4, 14, 28, 51) have no meaningful line
        // range, so they must resolve to None here too, same as line_for.
        let exact: &[(u32, Option<(u32, u32)>)] = &[
            (0, Some((0, 2))),
            (2, Some((2, 4))),
            (4, None), // hidden
            (6, Some((6, 7))),
            (7, Some((7, 14))),
            (14, None), // hidden
            (17, Some((17, 18))),
            (18, Some((18, 27))),
            (27, Some((27, 28))),
            (28, None), // hidden
            (30, Some((30, 31))),
            (31, Some((31, 40))),
            (40, Some((40, 41))),
            (41, Some((41, 42))),
            (42, Some((42, 46))), // back-edge onto line 2's own range
            (46, Some((46, 51))),
            (51, None), // hidden
            (54, Some((54, u32::MAX))), // last sequence point in the method
        ];
        for &(offset, expected) in exact {
            assert_eq!(
                pdb.step_range_for(token, offset),
                expected,
                "offset {offset}: expected {expected:?}"
            );
        }

        // Coverage semantics, mirroring line_for's equivalent assertions:
        // an offset strictly BETWEEN two recorded points resolves to the
        // EARLIER point's own (start, end) range, not a range starting at
        // the query offset itself — this is exactly what makes StepRange
        // correct: arming it with this range steps until execution leaves
        // the CURRENT line, regardless of which IL instruction inside that
        // line we happened to query from.
        assert_eq!(pdb.step_range_for(token, 3), Some((2, 4)));
        assert_eq!(pdb.step_range_for(token, 13), Some((7, 14)));
        assert_eq!(pdb.step_range_for(token, 16), None); // still hidden (14..17)
        assert_eq!(pdb.step_range_for(token, 53), None); // still hidden (51..54)
        assert_eq!(pdb.step_range_for(token, 1000), Some((54, u32::MAX)));
    }

    #[test]
    fn step_range_for_falls_back_to_none_for_unknown_method_or_missing_data() {
        let pdb = parse(BRANCHING_LOOP_FIXTURE).expect("fixture should parse");
        assert_eq!(pdb.step_range_for(0x0600_0099, 0), None);
        let older = parse(FIXTURE).expect("older fixture should still parse");
        assert_eq!(older.step_range_for(0x0600_0001, 0).is_some(), true);
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
