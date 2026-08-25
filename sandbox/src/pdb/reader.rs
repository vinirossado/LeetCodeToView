// Low-level binary-reading primitives shared by the rest of the `pdb` module
// — no Portable-PDB-specific semantics here, just byte-level plumbing
// (little-endian integers, null-terminated strings, ECMA-335 compressed
// integers, and #Blob heap entries). See `pdb/mod.rs`'s module doc comment
// for the overall format context.

pub(super) struct Reader<'a> {
    pub(super) data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(super) fn u16(&self, offset: usize) -> Option<u16> {
        self.data.get(offset..offset + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    pub(super) fn u32(&self, offset: usize) -> Option<u32> {
        self.data
            .get(offset..offset + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(super) fn idx(&self, offset: usize, wide: bool) -> Option<u32> {
        if wide {
            self.u32(offset)
        } else {
            self.u16(offset).map(|v| v as u32)
        }
    }

    /// Null-terminated string starting at `offset`, e.g. a stream name or a
    /// #Strings heap entry (both are UTF-8/ASCII, null-terminated).
    pub(super) fn c_str(&self, offset: usize) -> Option<&'a str> {
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
pub(super) fn read_compressed_uint(data: &[u8], pos: usize) -> Option<(u32, usize)> {
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
pub(super) fn read_compressed_int(data: &[u8], pos: usize) -> Option<(i32, usize)> {
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
pub(super) fn blob_slice(data: &[u8], heap_base: usize, idx: u32) -> Option<&[u8]> {
    let start = heap_base.checked_add(idx as usize)?;
    let (len, n) = read_compressed_uint(data, start)?;
    let content_start = start.checked_add(n)?;
    let content_end = content_start.checked_add(len as usize)?;
    data.get(content_start..content_end)
}
