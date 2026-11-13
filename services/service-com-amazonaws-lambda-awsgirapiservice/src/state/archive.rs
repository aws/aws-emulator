// SPDX-License-Identifier: CC0-1.0

//! How large a stored .zip archive is once EXPANDED.
//!
//! A layer version and a deployment package are both ".zip file archive"s, and Lambda
//! reports two sizes for one: the size of the archive as stored, and the size of its
//! CONTENTS once unzipped — the quantity the unzipped-deployment-package limit
//! ([`super::CODE_SIZE_UNZIPPED_LIMIT`]) is measured against. Only the first is the length
//! of the bytes the store holds, so the second is read here.
//!
//! It is read out of the archive's own directory, where every entry DECLARES the size of
//! its contents: nothing is inflated, so the read costs one directory walk and holds no
//! entry's bytes. The declared sizes are the caller's own numbers, which is why the walk is
//! bounded ([`super::execution::package::MAX_PACKAGE_ENTRIES`] entries, and saturating
//! arithmetic over them) rather than trusted to be small.
//!
//! An archive this emulator holds no bytes for — an S3 object it does not resolve, a layer
//! Lambda itself provides — measures 0, exactly as its STORED size does: nothing is
//! reported that was not computed from bytes this emulator has.

use super::execution::package::MAX_PACKAGE_ENTRIES;

/// The number of bytes the entries of `zip_bytes` hold once expanded: the sum of the
/// content size every entry of the archive declares.
///
/// The walk stops after [`MAX_PACKAGE_ENTRIES`] entries, so an archive with more entries
/// than that is measured as the sum of its first [`MAX_PACKAGE_ENTRIES`]: the declared
/// sizes are the caller's, and a directory of any length cannot be allowed to make an
/// arbitrarily long read.
///
/// 0 when this emulator holds no bytes for the archive, and 0 when the bytes it holds are
/// not a readable .zip directory — the same answer the stored size takes in both cases,
/// because neither is an expanded size this emulator measured.
#[must_use]
pub fn unzipped_bytes(zip_bytes: Option<&[u8]>) -> i64 {
    let Some(bytes) = zip_bytes else {
        return 0;
    };
    let Ok(mut archive) = ::zip::ZipArchive::new(::std::io::Cursor::new(bytes)) else {
        return 0;
    };
    let entries = archive.len().min(MAX_PACKAGE_ENTRIES);
    let mut unzipped: i64 = 0;
    for index in 0..entries {
        // The RAW entry, so an entry stored with a compression method this build links no
        // decoder for still contributes the size it declares: the number is read off the
        // directory and nothing is decoded.
        let Ok(entry) = archive.by_index_raw(index) else {
            continue;
        };
        unzipped = unzipped.saturating_add(i64::try_from(entry.size()).unwrap_or(i64::MAX));
    }
    unzipped
}

/// Whether the archive `zip_bytes` holds expands to [`super::CODE_SIZE_UNZIPPED_LIMIT`] or
/// more — the bound the refusal states, so the ceiling itself is over it and one byte under
/// it is not.
///
/// Measured from the archive's own directory ([`unzipped_bytes`]), so a package whose stored
/// bytes are small and whose CONTENTS are not is judged on what it expands to. An archive
/// this emulator holds no bytes for, or cannot read, measures 0 and is not over the ceiling:
/// nothing is refused on a size that was not computed.
#[must_use]
pub fn is_over_the_unzipped_limit(zip_bytes: Option<&[u8]>) -> bool {
    unzipped_bytes(zip_bytes) >= super::CODE_SIZE_UNZIPPED_LIMIT
}

/// The sentence Lambda refuses a .zip package whose expanded contents reach the published
/// unzipped ceiling with: the ceiling the package was measured against and nothing of the
/// request.
///
/// The ONE wording for that ceiling, whichever write named the package and wherever its
/// bytes came from — the inline upload, and the object Amazon S3 handed over
/// ([`super::s3_source`], which reports it for an object whose reported LENGTH is already
/// past the ceiling).
#[must_use]
pub fn unzipped_limit_message() -> String {
    format!(
        "Unzipped size must be smaller than {} bytes",
        super::CODE_SIZE_UNZIPPED_LIMIT
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bytes of a .zip archive holding `entries`, written in memory.
    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = ::std::io::Cursor::new(Vec::new());
        {
            let mut writer = ::zip::ZipWriter::new(&mut buffer);
            let options: ::zip::write::FileOptions<'_, '_, ()> =
                ::zip::write::FileOptions::default();
            for (name, bytes) in entries {
                writer.start_file(*name, options).expect("a new entry");
                ::std::io::Write::write_all(&mut writer, bytes).expect("the entry's bytes");
            }
            writer.finish().expect("a finished archive");
        }
        buffer.into_inner()
    }

    /// RULE: the expanded size of an archive is the sum of its entries' content sizes,
    /// whatever those entries compress to — and an archive whose bytes this emulator does
    /// not hold, or cannot read, measures 0.
    ///
    /// The rows span what the rule generalises over: the number of entries, an entry whose
    /// contents compress to far less than they expand to, an empty entry, and the two ways
    /// there is nothing to measure.
    #[test]
    fn an_archives_expanded_size_is_what_its_entries_declare() {
        let compressible = vec![b'0'; 64 * 1024];
        let held: &[(&str, Vec<u8>, i64)] = &[
            ("no entries at all", archive(&[]), 0),
            ("one empty entry", archive(&[("empty.txt", b"")]), 0),
            ("one entry", archive(&[("a.txt", b"0123456789")]), 10),
            (
                "several entries",
                archive(&[("a.txt", b"0123456789"), ("b/c.txt", b"abc")]),
                13,
            ),
            (
                "an entry that compresses to far less than it expands to",
                archive(&[("big.txt", &compressible)]),
                65_536,
            ),
        ];
        for (case, bytes, expected) in held {
            let measured = unzipped_bytes(Some(bytes));
            assert_eq!(
                measured, *expected,
                "case {case:?}: expected {expected} expanded bytes, got {measured}"
            );
        }

        for (case, bytes) in [
            ("no bytes held", None),
            ("bytes that are no archive", Some(b"not a zip at all".as_slice())),
            ("empty bytes", Some(b"".as_slice())),
        ] {
            let measured = unzipped_bytes(bytes);
            assert_eq!(
                measured, 0,
                "case {case:?}: expected 0 expanded bytes, got {measured}"
            );
        }
    }

    /// An archive with more entries than the walk reads is measured over the entries it
    /// read, which is the bound this module states rather than a measurement of the whole
    /// directory.
    #[test]
    fn an_archive_past_the_entry_bound_is_measured_over_the_entries_read() {
        let names: Vec<String> = (0..MAX_PACKAGE_ENTRIES + 16)
            .map(|index| format!("e{index}.bin"))
            .collect();
        let entries: Vec<(&str, &[u8])> = names
            .iter()
            .map(|name| (name.as_str(), b"x" as &[u8]))
            .collect();
        let held = archive(&entries);
        let measured = unzipped_bytes(Some(&held));
        let expected = i64::try_from(MAX_PACKAGE_ENTRIES).unwrap_or(i64::MAX);
        assert_eq!(
            measured, expected,
            "expected the first {MAX_PACKAGE_ENTRIES} entries of one byte each to measure \
             {expected}, got {measured}"
        );
    }
}
