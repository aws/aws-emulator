// SPDX-License-Identifier: CC0-1.0

//! Packfiles: the format git sends objects in, read and written.
//!
//! A `git push` arrives as a packfile in the `git-receive-pack` request body, and
//! a `git clone` or `git fetch` is answered with one in the `git-upload-pack`
//! response. This module is the only place that format is parsed or produced:
//! [`read_pack`] turns a packfile into the objects this emulator stores, and
//! [`write_pack`] turns stored objects back into a packfile.
//!
//! Everything is bounded, and bounded on the PRODUCT rather than per item. A
//! pack larger than [`MAX_PACK_BYTES`], holding more than [`MAX_PACK_OBJECTS`]
//! entries, carrying an object larger than [`MAX_OBJECT_BYTES`], inflating in
//! total to more than [`MAX_PACK_INFLATED_BYTES`], or taking longer than
//! [`MAX_PACK_SECONDS`] is refused rather than read to the end — and no buffer is
//! ever sized from a length the CLIENT declared, only from the bytes actually
//! produced. So a small request cannot drive the emulator's memory or CPU (a "zip
//! bomb" in pack form, whose pack shape is a few bytes of entry header per
//! megabyte of claimed output).

use std::collections::BTreeMap;

use flate2::{Compression, Decompress, FlushDecompress, Status};

use crate::state::git_object::{self, Kind, Object, OBJECT_ID_BYTES};

/// The largest packfile this emulator accepts or produces, in bytes.
///
/// The emulator's OWN bound, not a git or AWS limit: the whole pack is buffered
/// in memory, so this is what keeps one request's cost bounded.
pub const MAX_PACK_BYTES: usize = 8 * 1024 * 1024;

/// The most entries one packfile may declare. The emulator's own bound.
pub const MAX_PACK_OBJECTS: usize = 20_000;

/// The largest single object this emulator inflates from a pack, in bytes. The
/// emulator's own bound, equal to the whole pack it accepts ([`MAX_PACK_BYTES`]),
/// so one entry can be as large as one request.
///
/// It is deliberately ABOVE the service's own 6 MB API limit
/// ([`crate::state::MAX_RETRIEVABLE_OBJECT_BYTES`]): a git client is the documented
/// way to put content past that limit into a repository, and an object it pushed
/// above it is held and served to git while a read of it through the API is refused.
/// A bound at the API limit would make that refusal unreachable.
pub const MAX_OBJECT_BYTES: usize = MAX_PACK_BYTES;

/// The total inflated bytes one pack may produce, across every entry and every
/// delta it expands.
///
/// The emulator's own bound, and the one that makes the others hold as a PRODUCT:
/// [`MAX_OBJECT_BYTES`] bounds one entry and [`MAX_PACK_OBJECTS`] bounds the
/// count, but their product is not a bound anybody chose, and a delta entry costs
/// a handful of bytes to write however large its result. It is four times
/// [`MAX_PACK_BYTES`], so a pack may expand four-fold and no further — comfortably
/// above what a real push of this emulator's bounded repositories produces (a
/// repository holds at most [`crate::state::MAX_OBJECT_BYTES_PER_REPOSITORY`]).
pub const MAX_PACK_INFLATED_BYTES: usize = 4 * MAX_PACK_BYTES;

/// How long reading one pack may take before it is refused. The emulator's own
/// bound, and the figure the workspace's other caller-driven CPU work uses (the
/// Step Functions interpreter's `MAX_INTERPRET_DURATION`): the byte budgets above
/// bound the work, and this bounds the time it may take on any machine, so
/// neither the thread it runs on nor the store lock it runs under is held for an
/// unbounded span. The work is also moved off the async worker — see
/// `crate::service`'s `off_the_async_worker`.
pub const MAX_PACK_SECONDS: u64 = 2;

/// How much output an inflating buffer reserves at a time.
///
/// Allocation tracks the bytes actually produced rather than the size an entry
/// header DECLARES: a header that claims megabytes costs a client five bytes to
/// write, so reserving from it hands a tiny request an enormous allocation.
const OUTPUT_CHUNK: usize = 16 * 1024;

/// What one pack read may still spend: output bytes, and time.
///
/// One budget is threaded through every growth site of a single [`read_pack`] —
/// each entry's inflate and each delta's expansion — so the SUM is what is
/// bounded, not each term. The deadline is the crate's one
/// [`crate::state::Deadline`], the same reading of "out of time" the push
/// projection's and a merge's budgets use.
struct Budget {
    bytes_left: usize,
    deadline: crate::state::Deadline,
}

impl Budget {
    fn new() -> Self {
        Self {
            bytes_left: MAX_PACK_INFLATED_BYTES,
            deadline: crate::state::Deadline::in_seconds(MAX_PACK_SECONDS),
        }
    }

    /// Charge `bytes` of output to the budget, refusing the pack when it would go
    /// past its total or has run out of time.
    fn spend(&mut self, bytes: usize) -> Result<(), PackError> {
        if self.deadline.passed() {
            return fail(format!(
                "the pack took longer than the {MAX_PACK_SECONDS} seconds this emulator spends reading one"
            ));
        }
        match self.bytes_left.checked_sub(bytes) {
            Some(left) => {
                self.bytes_left = left;
                Ok(())
            }
            None => fail(format!(
                "the pack inflates past the {MAX_PACK_INFLATED_BYTES} bytes this emulator accepts in total"
            )),
        }
    }

    /// What the budget would still allow one object to produce.
    fn remaining(&self) -> usize {
        self.bytes_left
    }
}

/// Why a packfile could not be read. The text becomes the `unpack` line of the
/// git report-status response, which is what a pushing client prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackError(pub String);

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn fail<T>(message: impl Into<String>) -> Result<T, PackError> {
    Err(PackError(message.into()))
}

/// What one packfile entry carries before its delta (if any) is applied.
enum Entry {
    /// A whole object.
    Whole(Object),
    /// A delta against another entry of this same pack, by its offset.
    Offset { base: usize, delta: Vec<u8> },
    /// A delta against an object named directly — either an earlier entry of
    /// this pack or an object the receiver already holds (a "thin" pack).
    Reference { base: String, delta: Vec<u8> },
}

impl Entry {
    /// This entry with its delta payload put back, so an unresolved delta is
    /// carried to the next resolution round by MOVE rather than by copy.
    fn with_delta(self, payload: Vec<u8>) -> Self {
        match self {
            Self::Offset { base, .. } => Self::Offset {
                base,
                delta: payload,
            },
            Self::Reference { base, .. } => Self::Reference {
                base,
                delta: payload,
            },
            whole => whole,
        }
    }
}

/// Read every object out of a packfile.
///
/// `held` resolves an object the pack deltas against but does not carry, which is
/// what makes a thin pack (the kind `git push` sends by default) readable: its
/// deltas are against objects the server already has.
pub fn read_pack(
    bytes: &[u8],
    held: &dyn Fn(&str) -> Option<Object>,
) -> Result<Vec<Object>, PackError> {
    if bytes.len() > MAX_PACK_BYTES {
        return fail(format!(
            "the pack is larger than the {MAX_PACK_BYTES} bytes this emulator accepts"
        ));
    }
    if bytes.len() < 12 + OBJECT_ID_BYTES || &bytes[..4] != b"PACK" {
        return fail("the request body is not a packfile");
    }
    let version = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    if version != 2 && version != 3 {
        return fail(format!("pack version {version} is not supported"));
    }
    let declared = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    if declared > MAX_PACK_OBJECTS {
        return fail(format!(
            "the pack declares {declared} objects, more than the {MAX_PACK_OBJECTS} this emulator accepts"
        ));
    }
    let body_end = bytes.len() - OBJECT_ID_BYTES;
    let trailer = &bytes[body_end..];
    if git_object::sha1_digest(&bytes[..body_end])[..] != trailer[..] {
        return fail("the pack checksum does not match its contents");
    }

    // PASS 1 — frame every entry and inflate it, without resolving deltas: a
    // delta's base may be any earlier entry, or (for a reference delta) one that
    // comes later or is already held. ONE budget covers both passes, so what is
    // bounded is everything this pack produces rather than each entry alone.
    let mut budget = Budget::new();
    let mut entries: Vec<(usize, Entry)> = Vec::new();
    let mut cursor = 12usize;
    for index in 0..declared {
        let offset = cursor;
        let (pack_type, size) = read_entry_header(bytes, &mut cursor)
            .ok_or_else(|| PackError(format!("entry {index} has a truncated header")))?;
        if size > MAX_OBJECT_BYTES {
            return fail(format!(
                "entry {index} declares {size} bytes, more than the {MAX_OBJECT_BYTES} this emulator accepts"
            ));
        }
        let entry = match pack_type {
            6 => {
                let distance = read_offset(bytes, &mut cursor)
                    .ok_or_else(|| PackError(format!("entry {index} has a truncated base offset")))?;
                let base = offset
                    .checked_sub(distance)
                    .ok_or_else(|| PackError(format!("entry {index} names a base before the pack")))?;
                Entry::Offset {
                    base,
                    delta: inflate(bytes, &mut cursor, size, index, &mut budget)?,
                }
            }
            7 => {
                let end = cursor + OBJECT_ID_BYTES;
                let name = bytes
                    .get(cursor..end)
                    .ok_or_else(|| PackError(format!("entry {index} has a truncated base name")))?;
                cursor = end;
                Entry::Reference {
                    base: ::hex::encode(name),
                    delta: inflate(bytes, &mut cursor, size, index, &mut budget)?,
                }
            }
            whole => {
                let kind = Kind::from_pack_type(whole).ok_or_else(|| {
                    PackError(format!("entry {index} has unknown object type {whole}"))
                })?;
                let data = inflate(bytes, &mut cursor, size, index, &mut budget)?;
                Entry::Whole(Object::new(kind, data))
            }
        };
        // Every entry's inflated length must be the one its header declared —
        // for a delta entry as much as for a whole object, so a header cannot
        // claim one size and carry another.
        let inflated = match &entry {
            Entry::Whole(object) => object.data.len(),
            Entry::Offset { delta, .. } | Entry::Reference { delta, .. } => delta.len(),
        };
        if inflated != size {
            return fail(format!(
                "entry {index} inflated to {inflated} bytes but declared {size}"
            ));
        }
        entries.push((offset, entry));
    }
    if cursor != body_end {
        return fail(format!(
            "the pack has {} trailing bytes after its {declared} objects",
            body_end.saturating_sub(cursor)
        ));
    }

    // PASS 2 — resolve deltas, repeating while any entry resolves. Each round
    // resolves at least one entry or the remainder cannot be resolved at all, so
    // this terminates in at most one round per entry.
    let mut resolved: BTreeMap<usize, Object> = BTreeMap::new();
    let mut by_name: BTreeMap<String, usize> = BTreeMap::new();
    let mut pending: Vec<(usize, Entry)> = Vec::new();
    for (offset, entry) in entries {
        match entry {
            Entry::Whole(object) => {
                by_name.insert(object.id(), offset);
                resolved.insert(offset, object);
            }
            delta => pending.push((offset, delta)),
        }
    }
    while !pending.is_empty() {
        let mut still_pending: Vec<(usize, Entry)> = Vec::new();
        let before = pending.len();
        // Each entry is taken BY VALUE, so an unresolved delta is carried to the
        // next round without copying its payload: a chain that resolves one entry
        // per round therefore costs a map lookup per entry per round, and no
        // memcpy. The budget's deadline bounds those rounds in time.
        for (offset, entry) in pending {
            let (base, delta, unresolved) = match entry {
                Entry::Offset { base, delta } => (
                    resolved.get(&base).cloned(),
                    delta,
                    Entry::Offset { base, delta: Vec::new() },
                ),
                Entry::Reference { base, delta } => (
                    by_name
                        .get(&base)
                        .and_then(|offset| resolved.get(offset))
                        .cloned()
                        .or_else(|| held(&base)),
                    delta,
                    Entry::Reference { base, delta: Vec::new() },
                ),
                // A whole entry was resolved before this loop began.
                Entry::Whole(object) => {
                    by_name.insert(object.id(), offset);
                    resolved.insert(offset, object);
                    continue;
                }
            };
            match base {
                Some(base) => {
                    let data = apply_delta(&base.data, &delta, offset, &mut budget)?;
                    let object = Object::new(base.kind, data);
                    by_name.insert(object.id(), offset);
                    resolved.insert(offset, object);
                }
                None => still_pending.push((offset, unresolved.with_delta(delta))),
            }
        }
        if still_pending.len() == before {
            return fail("the pack deltas against objects it does not carry");
        }
        pending = still_pending;
    }

    Ok(resolved.into_values().collect())
}

/// A packfile carrying `objects`, in the order given.
pub fn write_pack(objects: &[Object]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"PACK");
    out.extend_from_slice(&2u32.to_be_bytes());
    out.extend_from_slice(&u32::try_from(objects.len()).unwrap_or(u32::MAX).to_be_bytes());
    for object in objects {
        write_entry_header(&mut out, object.kind, object.data.len());
        out.extend_from_slice(&deflate(&object.data));
    }
    let checksum = git_object::sha1_digest(&out);
    out.extend_from_slice(&checksum);
    out
}

/// The entry header: the object's type in bits 4-6 of the first byte and its
/// inflated size in a little-endian 7-bit continuation encoding.
fn write_entry_header(out: &mut Vec<u8>, kind: Kind, size: usize) {
    let mut remaining = size >> 4;
    #[allow(clippy::cast_possible_truncation)]
    let mut byte = (kind.pack_type() << 4) | (size & 0x0f) as u8;
    while remaining > 0 {
        out.push(byte | 0x80);
        #[allow(clippy::cast_possible_truncation)]
        {
            byte = (remaining & 0x7f) as u8;
        }
        remaining >>= 7;
    }
    out.push(byte);
}

/// Read an entry header, reporting `(type, inflated size)`.
fn read_entry_header(bytes: &[u8], cursor: &mut usize) -> Option<(u8, usize)> {
    let first = *bytes.get(*cursor)?;
    *cursor += 1;
    let pack_type = (first >> 4) & 0x07;
    let mut size = usize::from(first & 0x0f);
    let mut shift = 4u32;
    let mut byte = first;
    while byte & 0x80 != 0 {
        byte = *bytes.get(*cursor)?;
        *cursor += 1;
        if shift > 56 {
            return None;
        }
        size |= usize::from(byte & 0x7f) << shift;
        shift += 7;
    }
    Some((pack_type, size))
}

/// Read an offset-delta's distance back to its base: git's own big-endian
/// continuation encoding, which is not the same as the size encoding above.
fn read_offset(bytes: &[u8], cursor: &mut usize) -> Option<usize> {
    let mut byte = *bytes.get(*cursor)?;
    *cursor += 1;
    let mut value = usize::from(byte & 0x7f);
    while byte & 0x80 != 0 {
        byte = *bytes.get(*cursor)?;
        *cursor += 1;
        value = value
            .checked_add(1)
            .and_then(|value| value.checked_mul(128))?
            | usize::from(byte & 0x7f);
    }
    Some(value)
}

/// Inflate one zlib stream starting at `cursor`, advancing it past exactly the
/// bytes the stream consumed.
///
/// `expected` is the size the entry header declared, which is a value the CLIENT
/// chose: it caps how much this may produce (together with `budget`) but is never
/// what the buffer is sized from, so a header claiming megabytes cannot reserve
/// them. The buffer grows in [`OUTPUT_CHUNK`] steps as bytes are actually
/// produced, every one of them charged to `budget`.
fn inflate(
    bytes: &[u8],
    cursor: &mut usize,
    expected: usize,
    index: usize,
    budget: &mut Budget,
) -> Result<Vec<u8>, PackError> {
    let ceiling = expected.min(MAX_OBJECT_BYTES);
    let mut decompressor = Decompress::new(true);
    let mut out: Vec<u8> = Vec::with_capacity(ceiling.clamp(1, OUTPUT_CHUNK));
    let input = bytes
        .get(*cursor..)
        .ok_or_else(|| PackError(format!("entry {index} is truncated")))?;
    // One round either consumes input or grows the output buffer, so the cap is
    // generous: it only stops a decompressor that reports neither.
    for _ in 0..(2 * (MAX_OBJECT_BYTES / OUTPUT_CHUNK + input.len() + 2)) {
        let consumed = usize::try_from(decompressor.total_in()).unwrap_or(usize::MAX);
        let before_out = out.len();
        let status = decompressor
            .decompress_vec(
                input.get(consumed..).unwrap_or_default(),
                &mut out,
                FlushDecompress::None,
            )
            .map_err(|error| PackError(format!("entry {index} is not deflated data: {error}")))?;
        // Charge what this round produced before deciding to go round again.
        budget.spend(out.len() - before_out)?;
        if status == Status::StreamEnd {
            *cursor += usize::try_from(decompressor.total_in()).unwrap_or(0);
            out.shrink_to_fit();
            return Ok(out);
        }
        if out.len() >= ceiling {
            return fail(format!(
                "entry {index} inflates past the {ceiling} bytes it declared"
            ));
        }
        let made_progress = out.len() > before_out;
        if out.len() == out.capacity() {
            // The output buffer is full and the stream has not ended: grow it by
            // one chunk, never past what this entry may produce.
            out.reserve(OUTPUT_CHUNK.min(ceiling - out.len()).max(1));
        } else if !made_progress && consumed >= input.len() {
            return fail(format!("entry {index} has a truncated deflate stream"));
        } else if !made_progress && status == Status::BufError {
            return fail(format!("entry {index} has a corrupt deflate stream"));
        }
    }
    fail(format!("entry {index} could not be inflated"))
}

/// Deflate one object's payload the way a packfile carries it.
fn deflate(data: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), Compression::default());
    // Writing to a `Vec` cannot fail, and `finish` only reports a write error.
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_default()
}

/// Apply a git delta to its base, producing the object's payload.
///
/// The result size the delta header declares is the CLIENT's value: it is checked
/// against what the budget still allows before anything is reserved, and the
/// buffer grows in [`OUTPUT_CHUNK`] steps as bytes are produced, so a header
/// claiming megabytes reserves nothing.
fn apply_delta(
    base: &[u8],
    delta: &[u8],
    offset: usize,
    budget: &mut Budget,
) -> Result<Vec<u8>, PackError> {
    let malformed = || PackError(format!("the delta at offset {offset} does not apply"));
    let mut cursor = 0usize;
    let base_size = read_delta_size(delta, &mut cursor).ok_or_else(malformed)?;
    if base_size != base.len() {
        return Err(malformed());
    }
    let result_size = read_delta_size(delta, &mut cursor).ok_or_else(malformed)?;
    if result_size > MAX_OBJECT_BYTES {
        return fail(format!(
            "the delta at offset {offset} expands past the {MAX_OBJECT_BYTES} bytes this emulator accepts"
        ));
    }
    if result_size > budget.remaining() {
        return fail(format!(
            "the pack inflates past the {MAX_PACK_INFLATED_BYTES} bytes this emulator accepts in total"
        ));
    }
    budget.spend(result_size)?;
    let mut out: Vec<u8> = Vec::with_capacity(result_size.clamp(1, OUTPUT_CHUNK));
    while cursor < delta.len() {
        let command = delta[cursor];
        cursor += 1;
        if command & 0x80 != 0 {
            // Copy from the base: up to four offset bytes then three size bytes,
            // each present only when its bit is set.
            let mut from = 0usize;
            for shift in 0..4u32 {
                if command & (1 << shift) != 0 {
                    from |= usize::from(*delta.get(cursor).ok_or_else(malformed)?) << (8 * shift);
                    cursor += 1;
                }
            }
            let mut size = 0usize;
            for shift in 0..3u32 {
                if command & (0x10 << shift) != 0 {
                    size |= usize::from(*delta.get(cursor).ok_or_else(malformed)?) << (8 * shift);
                    cursor += 1;
                }
            }
            if size == 0 {
                // Git writes a zero copy size to mean 0x10000.
                size = 0x10000;
            }
            let end = from.checked_add(size).ok_or_else(malformed)?;
            out.extend_from_slice(base.get(from..end).ok_or_else(malformed)?);
        } else {
            let size = usize::from(command & 0x7f);
            if size == 0 {
                return Err(malformed());
            }
            let end = cursor.checked_add(size).ok_or_else(malformed)?;
            out.extend_from_slice(delta.get(cursor..end).ok_or_else(malformed)?);
            cursor += size;
        }
        if out.len() > result_size {
            return Err(malformed());
        }
    }
    if out.len() != result_size {
        return Err(malformed());
    }
    out.shrink_to_fit();
    Ok(out)
}

/// The little-endian 7-bit size encoding a delta header uses.
fn read_delta_size(delta: &[u8], cursor: &mut usize) -> Option<usize> {
    let mut value = 0usize;
    let mut shift = 0u32;
    loop {
        let byte = *delta.get(*cursor)?;
        *cursor += 1;
        if shift > 56 {
            return None;
        }
        value |= usize::from(byte & 0x7f) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
}
