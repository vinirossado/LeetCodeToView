// SequencePoints blob parsing — the IL-offset -> source-line mapping. See
// `pdb/mod.rs`'s module doc comment for the two-capability overview this
// piece fits into.

use super::reader::{read_compressed_int, read_compressed_uint};

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
pub(super) struct SequencePointEntry {
    pub(super) il_offset: u32,
    pub(super) line: Option<u32>,
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
pub(super) fn parse_sequence_points_blob(blob: &[u8], document_is_nil: bool) -> Vec<SequencePointEntry> {
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
